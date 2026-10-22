//! Content security policy for served publication documents.
//!
//! Every profile emits the same directive list in the same order; they differ
//! only in which origin token fills each source list. One builder owns that
//! text so a directive cannot drift between profiles.

use super::PublicationResponseProfile;
use crate::reader::{content_agent_url, reader_font_base_url};
use crate::{
    ResolvedActiveContentPolicy, ResolvedPublicationPolicy, ResolvedRemoteResourcesPolicy,
};

pub fn measurement_document_csp(reader_origin: &str, nonce: &str) -> String {
    let reader_font_base = reader_font_base_url(reader_origin);
    let content_agent = content_agent_url(reader_origin);
    format!(
        "default-src 'none'; script-src 'nonce-{nonce}' {content_agent}; script-src-attr 'none'; style-src 'self' 'unsafe-inline' {reader_font_base}/reader-serif.css; img-src 'self' data:; font-src 'self' data: {reader_font_base}/files/; media-src 'self' data:; connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors {reader_origin}; sandbox allow-scripts allow-same-origin"
    )
}

pub fn measurement_dependency_csp() -> String {
    "default-src 'none'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"
        .to_owned()
}

pub fn publication_csp_for_profile(
    profile: PublicationResponseProfile,
    reader_origin: &str,
    foreground_origin: &str,
    nested_origin: &str,
    policy: ResolvedPublicationPolicy,
    content_script_nonce: Option<&str>,
) -> String {
    match profile {
        PublicationResponseProfile::SpineLevel => {
            publication_csp(reader_origin, nested_origin, policy, content_script_nonce)
        }
        PublicationResponseProfile::ContainerConstrained => container_constrained_csp(
            reader_origin,
            foreground_origin,
            nested_origin,
            policy,
            content_script_nonce,
            None,
        ),
    }
}

/// How one response profile names its own origin and its neighbours in the
/// publication content security policy.
///
/// The two profiles differ only in these tokens: the directive list, the
/// policy-driven script, sandbox, and remote-source choices, and the ordering
/// are identical, so they share one builder.
pub(crate) struct CspProfile<'a> {
    /// What the served document calls its own origin in a source list.
    pub(crate) own: &'a str,
    pub(crate) frame_and_child: &'a str,
    pub(crate) frame_ancestors: String,
    /// The foreground frames nested content, so only it names a frame target.
    pub(crate) nests_frames: bool,
    /// The nested profile is cross-origin from its own resources, so it must
    /// admit `data:` for fonts and media where the foreground need not.
    pub(crate) data_fonts_and_media: bool,
    /// The foreground may connect to and spawn workers from its own origin.
    pub(crate) self_connect_and_workers: bool,
    pub(crate) sandbox_same_origin: bool,
    pub(crate) auxiliary_script: Option<&'a str>,
}

pub(crate) fn publication_csp_from(
    reader_origin: &str,
    profile: &CspProfile<'_>,
    policy: ResolvedPublicationPolicy,
    content_script_nonce: Option<&str>,
) -> String {
    let reader_font_base = reader_font_base_url(reader_origin);
    let content_agent = content_agent_url(reader_origin);
    let CspProfile {
        own,
        frame_and_child,
        frame_ancestors,
        nests_frames,
        data_fonts_and_media,
        self_connect_and_workers,
        sandbox_same_origin,
        auxiliary_script,
    } = profile;
    let remote_sources = match policy.remote_resources {
        ResolvedRemoteResourcesPolicy::Allowed => " https:",
        ResolvedRemoteResourcesPolicy::Blocked => "",
    };
    let frame_remote = if *nests_frames { remote_sources } else { "" };
    let data_source = if *data_fonts_and_media { " data:" } else { "" };
    let script_src = match policy.active_content {
        ResolvedActiveContentPolicy::Enabled => format!(
            "{own} {content_agent}{} 'unsafe-inline'{remote_sources}",
            auxiliary_script
                .map(|url| format!(" {url}"))
                .unwrap_or_default()
        ),
        ResolvedActiveContentPolicy::Disabled => format!(
            "'nonce-{}' {content_agent}",
            content_script_nonce
                .expect("disabled active content must have an instrumentation nonce")
        ),
    };
    let script_src_attr = match policy.active_content {
        ResolvedActiveContentPolicy::Enabled => "'unsafe-inline'",
        ResolvedActiveContentPolicy::Disabled => "'none'",
    };
    let connect_src = match (policy.remote_resources, self_connect_and_workers) {
        (ResolvedRemoteResourcesPolicy::Allowed, true) => "'self' https: wss:",
        (ResolvedRemoteResourcesPolicy::Allowed, false) => "https: wss:",
        (ResolvedRemoteResourcesPolicy::Blocked, true) => "'self'",
        (ResolvedRemoteResourcesPolicy::Blocked, false) => "'none'",
    };
    let worker_src = match (policy.active_content, self_connect_and_workers) {
        (ResolvedActiveContentPolicy::Enabled, true) => "'self'",
        _ => "'none'",
    };
    let sandbox_origin = if *sandbox_same_origin {
        " allow-same-origin"
    } else {
        ""
    };
    let sandbox = match policy.active_content {
        ResolvedActiveContentPolicy::Enabled => {
            format!("allow-scripts{sandbox_origin} allow-forms allow-modals")
        }
        ResolvedActiveContentPolicy::Disabled => format!("allow-scripts{sandbox_origin}"),
    };
    format!(
        "default-src 'none'; base-uri 'none'; object-src {frame_and_child}{frame_remote}; form-action 'none'; \
         frame-src {frame_and_child}{frame_remote}; child-src {frame_and_child}{frame_remote}; \
         frame-ancestors {frame_ancestors}; script-src {script_src}; \
         script-src-attr {script_src_attr}; \
         style-src {own} 'unsafe-inline' {reader_font_base}/reader-serif.css{remote_sources}; \
         img-src {own} data:{remote_sources}; \
         font-src {own}{data_source} {reader_font_base}/files/{remote_sources}; \
         media-src {own}{data_source}{remote_sources}; connect-src {connect_src}; \
         worker-src {worker_src}; sandbox {sandbox}"
    )
}

/// The durable foreground publication origin.
pub fn publication_csp(
    reader_origin: &str,
    nested_origin: &str,
    policy: ResolvedPublicationPolicy,
    content_script_nonce: Option<&str>,
) -> String {
    publication_csp_from(
        reader_origin,
        &CspProfile {
            own: "'self'",
            frame_and_child: nested_origin,
            frame_ancestors: reader_origin.to_owned(),
            nests_frames: true,
            data_fonts_and_media: false,
            self_connect_and_workers: true,
            sandbox_same_origin: true,
            auxiliary_script: None,
        },
        policy,
        content_script_nonce,
    )
}

/// An opaque cross-origin container frame, which nests nothing further.
pub fn container_constrained_csp(
    reader_origin: &str,
    foreground_origin: &str,
    nested_origin: &str,
    policy: ResolvedPublicationPolicy,
    content_script_nonce: Option<&str>,
    auxiliary_script_url: Option<&str>,
) -> String {
    publication_csp_from(
        reader_origin,
        &CspProfile {
            own: nested_origin,
            frame_and_child: "'none'",
            frame_ancestors: format!("{foreground_origin} {reader_origin}"),
            nests_frames: false,
            data_fonts_and_media: true,
            self_connect_and_workers: false,
            sandbox_same_origin: false,
            auxiliary_script: auxiliary_script_url,
        },
        policy,
        content_script_nonce,
    )
}
