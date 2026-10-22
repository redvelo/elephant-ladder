/// The publication content security policy is a security boundary, so its exact
/// text is pinned for every profile and policy combination. A deliberate change
/// shows up as a reviewable diff (`cargo insta review`); an accidental one fails.
///
/// Asset digests are normalized. They are sums of built bundles, so they change
/// on every content-agent or font rebuild and would otherwise fail this test for
/// reasons unrelated to the policy. Each is replaced by its own exact value, so
/// the digest-pinned URL shape stays pinned and an unexpected digest elsewhere
/// would still show up as a diff.
#[test]
fn publication_csp_text_is_exact_for_every_profile_and_policy() {
    use crate::mount::csp::{container_constrained_csp, publication_csp};
    use crate::policy::{
        ResolvedActiveContentPolicy as A, ResolvedPublicationPolicy,
        ResolvedRemoteResourcesPolicy as R,
    };
    use crate::reader::{content_agent_sha256, reader_font_bundle_id};

    const READER: &str = "http://reader.test";
    const FOREGROUND: &str = "http://foreground.test";
    const NESTED: &str = "http://nested.test";
    const AUXILIARY: &str = "http://aux.test/s.js";

    let mut rendered = String::new();
    for (active, remote) in [
        (A::Enabled, R::Allowed),
        (A::Enabled, R::Blocked),
        (A::Disabled, R::Allowed),
        (A::Disabled, R::Blocked),
    ] {
        let policy = ResolvedPublicationPolicy {
            active_content: active,
            remote_resources: remote,
        };
        for (label, csp) in [
            (
                "foreground",
                publication_csp(READER, NESTED, policy, Some("NONCE")),
            ),
            (
                "nested",
                container_constrained_csp(READER, FOREGROUND, NESTED, policy, Some("NONCE"), None),
            ),
            (
                "nested+auxiliary",
                container_constrained_csp(
                    READER,
                    FOREGROUND,
                    NESTED,
                    policy,
                    Some("NONCE"),
                    Some(AUXILIARY),
                ),
            ),
        ] {
            rendered.push_str(&format!("{label} | {active:?} | {remote:?}\n"));
            for directive in csp.split("; ") {
                rendered.push_str(&format!("    {directive}\n"));
            }
            rendered.push('\n');
        }
    }
    let rendered = rendered
        .replace(content_agent_sha256(), "[content-agent]")
        .replace(
            reader_font_bundle_id().expect("reader build must contain one versioned font bundle"),
            "[reader-fonts]",
        );
    insta::assert_snapshot!(rendered);
}
