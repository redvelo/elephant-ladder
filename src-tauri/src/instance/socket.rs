//! The Linux single-instance handoff: a second launch hands its links and publications to
//! the running process over a Unix domain socket, then exits.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use elephant_ladder_native_host::{ANNOTATION_LINK_SCHEME, MAX_ANNOTATION_LINK_BYTES};

use super::{LinkInbox, opened_publication_path};

const MAX_QUEUED_LINKS: usize = 16;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// Marks a publication the handing process copied for the receiver, which then owns it.
const STAGED_PREFIX: &str = "staged:";
/// Where a second launch leaves copies for the running instance, inside shared app storage.
const HANDED_DIRECTORY: &str = "handed";

impl LinkInbox {
    /// Files this process was launched with stay the user's own, so they are never removed.
    /// A launch names a book to read now; a drop hands books to the library.
    pub fn push(&self, argument: String) {
        if let Some(staged) = argument.strip_prefix(STAGED_PREFIX) {
            self.push_publication(staged.to_owned(), true, true);
        } else if argument.starts_with(&format!("{ANNOTATION_LINK_SCHEME}:")) {
            let mut arrivals = self.0.lock().expect("link inbox lock poisoned");
            if arrivals.links.len() < MAX_QUEUED_LINKS {
                arrivals.links.push(argument);
            }
        } else {
            self.push_publication(argument, false, true);
        }
    }
}

pub fn socket_path(app_data: &Path) -> PathBuf {
    app_data.join("instance.sock")
}

/// The arguments worth handing to the running instance: annotation links, and the paths of
/// publications a file manager or browser asked this application to open. A path is made
/// absolute here, because the second instance may not share this one's working directory.
pub fn link_arguments(arguments: impl IntoIterator<Item = String>) -> Vec<String> {
    arguments
        .into_iter()
        .filter_map(|argument| {
            if argument.len() > MAX_ANNOTATION_LINK_BYTES {
                return None;
            }
            if argument.starts_with(&format!("{ANNOTATION_LINK_SCHEME}:")) {
                return Some(argument);
            }
            opened_publication_path(&argument)
        })
        .collect()
}

/// Copies a publication into shared app storage so the running instance can read it after
/// this process exits. A sandbox hands each launch its own view of the file: the portal
/// document a Flatpak receives lives only as long as the launch that received it.
fn stage_for_running_instance(app_data: &Path, path: &str) -> Option<String> {
    let staging = app_data.join(HANDED_DIRECTORY);
    std::fs::create_dir_all(&staging).ok()?;
    // One launch stages a handful of files at most, so the launch and its order name them.
    static STAGED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = STAGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let copy = staging.join(format!("{}-{nanos}-{sequence}.epub", std::process::id()));
    std::fs::copy(path, &copy).ok()?;
    copy.into_os_string().into_string().ok()
}

/// Removes copies an earlier launch left behind. Only the instance that owns app storage runs
/// this, so anything still here was never read.
pub fn clear_handed_publications(app_data: &Path) {
    let _ = std::fs::remove_dir_all(app_data.join(HANDED_DIRECTORY));
}

/// Sends links to an already running instance. An empty message only asks it to come forward.
/// Publications travel as copies in shared storage, which the receiver reads and removes.
pub fn hand_off(socket: &Path, app_data: &Path, arguments: &[String]) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let lines: Vec<String> = arguments
        .iter()
        .filter_map(|argument| {
            if argument.starts_with(&format!("{ANNOTATION_LINK_SCHEME}:")) {
                return Some(argument.clone());
            }
            stage_for_running_instance(app_data, argument)
                .map(|staged| format!("{STAGED_PREFIX}{staged}"))
        })
        .collect();
    let mut message = lines.join("\n");
    message.push('\n');
    stream.write_all(message.as_bytes()).is_ok()
}

/// Must run only after this process owns the app storage lock, which makes a leftover socket stale.
pub fn listen(
    socket: &Path,
    inbox: LinkInbox,
    on_message: impl Fn() + Send + 'static,
) -> std::io::Result<()> {
    match std::fs::remove_file(socket) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new()
        .name("instance-links".to_owned())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
                    continue;
                }
                let limit = (MAX_ANNOTATION_LINK_BYTES + 1) * MAX_QUEUED_LINKS;
                for line in BufReader::new(stream.take(limit as u64)).lines() {
                    let Ok(line) = line else { break };
                    match line.strip_prefix(STAGED_PREFIX) {
                        // A staged copy is already this application's own file, so it is taken
                        // as handed rather than re-validated as a launch argument.
                        Some(path) if !path.is_empty() => inbox.push(line.clone()),
                        Some(_) => {}
                        None => {
                            for argument in link_arguments([line]) {
                                inbox.push(argument);
                            }
                        }
                    }
                }
                on_message();
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::HandedPublication;

    #[test]
    fn a_second_launch_hands_links_to_the_listening_instance() {
        let directory = tempfile::tempdir().unwrap();
        let socket = socket_path(directory.path());
        assert!(!hand_off(&socket, directory.path(), &[]));

        std::fs::write(&socket, b"stale").unwrap();
        let inbox = LinkInbox::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        listen(&socket, inbox.clone(), move || sender.send(()).unwrap()).unwrap();
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let links = link_arguments([
            "--flag".to_owned(),
            "elephant-ladder://annotation?id=a".to_owned(),
        ]);
        assert_eq!(links, ["elephant-ladder://annotation?id=a"]);
        assert!(hand_off(&socket, directory.path(), &links));
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(inbox.take(), links);

        assert!(hand_off(&socket, directory.path(), &[]));
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(inbox.take().is_empty());
    }

    #[test]
    fn opening_a_publication_hands_over_its_path_and_nothing_else() {
        let directory = tempfile::tempdir().unwrap();
        let book = directory.path().join("alice.epub");
        std::fs::write(&book, b"epub").unwrap();
        let elsewhere = directory.path().join("notes.txt");
        std::fs::write(&elsewhere, b"text").unwrap();
        let book_path = book.canonicalize().unwrap().display().to_string();

        let arguments = link_arguments([
            "--flag".to_owned(),
            "elephant-ladder://annotation?id=a".to_owned(),
            book.display().to_string(),
            format!("file://{book_path}"),
            elsewhere.display().to_string(),
            directory.path().display().to_string(),
            directory.path().join("missing.epub").display().to_string(),
        ]);
        assert_eq!(
            arguments,
            [
                "elephant-ladder://annotation?id=a".to_owned(),
                book_path.clone(),
                book_path.clone(),
            ]
        );

        let inbox = LinkInbox::default();
        for argument in arguments {
            inbox.push(argument);
        }
        assert_eq!(inbox.take(), ["elephant-ladder://annotation?id=a"]);
        assert_eq!(
            inbox.take_publications(),
            [
                HandedPublication {
                    path: book_path.clone(),
                    owned: false,
                    read: true,
                },
                HandedPublication {
                    path: book_path,
                    owned: false,
                    read: true,
                },
            ],
            "a launch argument is the user's own file, which this application never removes"
        );
        assert!(inbox.take().is_empty());
        assert!(inbox.take_publications().is_empty());
    }

    #[test]
    fn a_handed_publication_travels_as_a_copy_the_receiver_owns() {
        let directory = tempfile::tempdir().unwrap();
        let storage = directory.path().join("storage");
        std::fs::create_dir_all(&storage).unwrap();
        let book = directory.path().join("alice.epub");
        std::fs::write(&book, b"epub bytes").unwrap();
        let socket = socket_path(&storage);
        let inbox = LinkInbox::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        listen(&socket, inbox.clone(), move || sender.send(()).unwrap()).unwrap();

        let arguments = link_arguments([book.display().to_string()]);
        assert!(hand_off(&socket, &storage, &arguments));
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();

        let handed = inbox.take_publications();
        assert_eq!(handed.len(), 1);
        let handed = &handed[0];
        assert!(
            handed.owned,
            "the receiver owns the copy and removes it once read"
        );
        assert_ne!(handed.path, book.display().to_string());
        assert!(handed.path.starts_with(storage.to_str().unwrap()));
        assert_eq!(std::fs::read(&handed.path).unwrap(), b"epub bytes");
        assert!(book.exists(), "the user's own file is left where it is");

        clear_handed_publications(&storage);
        assert!(!PathBuf::from(&handed.path).exists());
    }
}
