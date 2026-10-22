use std::collections::HashMap;

const XHTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];

#[derive(Debug, Eq, PartialEq)]
pub struct InstrumentationError(&'static str);

impl std::fmt::Display for InstrumentationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for InstrumentationError {}

pub struct XhtmlInstrumentationOptions<'a> {
    pub(crate) content_agent_url: &'a str,
    pub(crate) nested_publication_base_url: Option<&'a str>,
    pub(crate) reader_font_stylesheet_url: Option<&'a str>,
    pub(crate) nonce: Option<&'a str>,
    pub(crate) auxiliary_script_url: Option<&'a str>,
    pub(crate) crossorigin: bool,
}

pub struct SvgInstrumentationOptions<'a> {
    pub(crate) content_agent_url: &'a str,
    pub(crate) nested_publication_base_url: Option<&'a str>,
    pub(crate) nonce: Option<&'a str>,
    pub(crate) auxiliary_script_url: Option<&'a str>,
    pub(crate) crossorigin: bool,
}

#[derive(Clone, Copy)]
enum DocumentKind {
    Xhtml,
    Svg,
}

impl DocumentKind {
    fn namespace(self) -> &'static str {
        match self {
            Self::Xhtml => XHTML_NAMESPACE,
            Self::Svg => SVG_NAMESPACE,
        }
    }

    fn root_name(self) -> &'static str {
        match self {
            Self::Xhtml => "html",
            Self::Svg => "svg",
        }
    }
}

struct StartTag {
    name: String,
    end: usize,
    self_closing: bool,
    namespace_declarations: HashMap<String, String>,
}

#[derive(Debug)]
enum ScanError {
    MissingRoot,
    MissingInsertion,
    Malformed(&'static str),
}

pub fn instrument_xhtml(
    source: &[u8],
    options: XhtmlInstrumentationOptions<'_>,
) -> Result<Vec<u8>, InstrumentationError> {
    let (text, bom_length) = decode(source)
        .map_err(|()| InstrumentationError("the XHTML document is not valid UTF-8"))?;
    let insertion = find_insertion(text, DocumentKind::Xhtml).map_err(|error| match error {
        ScanError::MissingRoot => InstrumentationError("the XHTML document has no root element"),
        ScanError::MissingInsertion => {
            InstrumentationError("the XHTML document has no direct head element")
        }
        ScanError::Malformed(message) => InstrumentationError(message),
    })?;
    let injection = encode_xhtml_injection(options)?;
    Ok(insert(source, bom_length + insertion, &injection))
}

pub fn instrument_svg(
    source: &[u8],
    options: SvgInstrumentationOptions<'_>,
) -> Result<Vec<u8>, InstrumentationError> {
    let (text, bom_length) =
        decode(source).map_err(|()| InstrumentationError("the SVG document is not valid UTF-8"))?;
    let insertion = find_insertion(text, DocumentKind::Svg).map_err(|error| match error {
        ScanError::MissingRoot | ScanError::MissingInsertion => {
            InstrumentationError("the SVG document has no root element")
        }
        ScanError::Malformed(message) => InstrumentationError(message),
    })?;
    let injection = encode_svg_injection(options)?;
    Ok(insert(source, bom_length + insertion, &injection))
}

fn decode(source: &[u8]) -> Result<(&str, usize), ()> {
    let bom_length = usize::from(source.starts_with(UTF8_BOM)) * UTF8_BOM.len();
    std::str::from_utf8(&source[bom_length..])
        .map(|text| (text, bom_length))
        .map_err(|_| ())
}

fn insert(source: &[u8], offset: usize, injection: &[u8]) -> Vec<u8> {
    let mut transformed = Vec::with_capacity(source.len().saturating_add(injection.len()));
    transformed.extend_from_slice(&source[..offset]);
    transformed.extend_from_slice(injection);
    transformed.extend_from_slice(&source[offset..]);
    transformed
}

fn find_insertion(text: &str, kind: DocumentKind) -> Result<usize, ScanError> {
    let mut stack: Vec<String> = Vec::new();
    let mut root_namespaces = None;
    let mut cursor = 0;
    let mut root_seen = false;

    while cursor < text.len() {
        // Character data is skipped, not judged: whether an entity resolves or a code point is
        // legal decides whether the browser parses this document, which it will decide for
        // itself. Refusing here would turn a page the browser renders into an error page.
        let Some(opening) = text[cursor..].find('<').map(|offset| cursor + offset) else {
            break;
        };

        if text[opening..].starts_with("<!--") {
            cursor = comment_end(text, opening)?;
            continue;
        }
        if text[opening..].starts_with("<![CDATA[") {
            if stack.is_empty() {
                return Err(ScanError::Malformed(
                    "the XML document has CDATA outside its root element",
                ));
            }
            cursor = required_terminator(text, opening + 9, "]]>")?;
            continue;
        }
        if text[opening..].starts_with("<?") {
            cursor = processing_instruction_end(text, opening)?;
            continue;
        }
        if text[opening..].starts_with("<!DOCTYPE") {
            cursor = doctype_end(text, opening)?;
            continue;
        }
        if text[opening..].starts_with("<!") {
            return Err(ScanError::Malformed(
                "the XML document has an unsupported declaration",
            ));
        }
        if text[opening..].starts_with("</") {
            let (name, end) = parse_closing_tag(text, opening)?;
            let current = stack.pop().ok_or(ScanError::Malformed(
                "the XML document has mismatched element tags",
            ))?;
            if current != name {
                return Err(ScanError::Malformed(
                    "the XML document has mismatched element tags",
                ));
            }
            if stack.is_empty() {
                return Err(ScanError::MissingInsertion);
            }
            cursor = end;
            continue;
        }

        let start = parse_start_tag(text, opening)?;
        if stack.is_empty() {
            if root_seen {
                return Err(ScanError::Malformed(
                    "the XML document has more than one root element before the insertion point",
                ));
            }
            let namespaces = apply_namespaces(None, &start.namespace_declarations);
            if local_name(&start.name) != kind.root_name()
                || namespace_for(&start.name, &namespaces) != kind.namespace()
            {
                return Err(ScanError::Malformed(
                    "the XML root element has the wrong name or namespace",
                ));
            }
            if start.self_closing {
                return Err(ScanError::Malformed(
                    "the XML root element must permit a child insertion",
                ));
            }
            root_seen = true;
            root_namespaces = Some(namespaces);
            if matches!(kind, DocumentKind::Svg) {
                return Ok(start.end);
            }
        } else if matches!(kind, DocumentKind::Xhtml) {
            if local_name(&start.name) == "script" {
                return Err(ScanError::Malformed(
                    "a script element precedes the XHTML insertion point",
                ));
            }
            if stack.len() == 1
                && local_name(&start.name) == "head"
                && namespace_for(
                    &start.name,
                    &apply_namespaces(root_namespaces.as_ref(), &start.namespace_declarations),
                ) == XHTML_NAMESPACE
            {
                if start.self_closing {
                    return Err(ScanError::Malformed(
                        "the XHTML head element must not be self-closing",
                    ));
                }
                return Ok(start.end);
            }
        }
        if !start.self_closing {
            stack.push(start.name);
        }
        cursor = start.end;
    }

    if !root_seen {
        return Err(ScanError::MissingRoot);
    }
    Err(ScanError::MissingInsertion)
}

fn parse_start_tag(text: &str, opening: usize) -> Result<StartTag, ScanError> {
    let mut cursor = opening + 1;
    let name = read_qualified_name(text, cursor)?.to_owned();
    cursor += name.len();
    let mut namespace_declarations = HashMap::new();
    loop {
        let before_whitespace = cursor;
        cursor = skip_whitespace(text, cursor);
        if text.as_bytes().get(cursor) == Some(&b'>') {
            return Ok(StartTag {
                name,
                end: cursor + 1,
                self_closing: false,
                namespace_declarations,
            });
        }
        if text.as_bytes().get(cursor..cursor + 2) == Some(b"/>") {
            return Ok(StartTag {
                name,
                end: cursor + 2,
                self_closing: true,
                namespace_declarations,
            });
        }
        if cursor == before_whitespace {
            return Err(ScanError::Malformed(
                "the XML document has a malformed start tag",
            ));
        }
        let attribute_name = read_qualified_name(text, cursor)?.to_owned();
        cursor += attribute_name.len();
        cursor = skip_whitespace(text, cursor);
        if text.as_bytes().get(cursor) != Some(&b'=') {
            return Err(ScanError::Malformed(
                "the XML document has an attribute without a value",
            ));
        }
        cursor = skip_whitespace(text, cursor + 1);
        let quote = *text.as_bytes().get(cursor).ok_or(ScanError::Malformed(
            "the XML document has an unterminated attribute",
        ))?;
        if quote != b'\'' && quote != b'"' {
            return Err(ScanError::Malformed(
                "the XML document has an unquoted attribute value",
            ));
        }
        let value_start = cursor + 1;
        let value_end = text.as_bytes()[value_start..]
            .iter()
            .position(|byte| *byte == quote)
            .map(|offset| value_start + offset)
            .ok_or(ScanError::Malformed(
                "the XML document has an unterminated attribute value",
            ))?;
        let value = &text[value_start..value_end];
        let namespace_prefix = if attribute_name == "xmlns" {
            Some("")
        } else {
            attribute_name.strip_prefix("xmlns:")
        };
        if let Some(prefix) = namespace_prefix {
            // Resolved, not validated: the mapping is needed to tell an XHTML `head` from a
            // foreign one. An illegal declaration is a document the parser will reject anyway,
            // and either way nothing can bind a name into the XHTML namespace by accident.
            namespace_declarations.insert(prefix.to_owned(), decode_namespace_value(value)?);
        }
        cursor = value_end + 1;
    }
}

fn parse_closing_tag(text: &str, opening: usize) -> Result<(String, usize), ScanError> {
    let mut cursor = opening + 2;
    let name = read_qualified_name(text, cursor)?.to_owned();
    cursor = skip_whitespace(text, cursor + name.len());
    if text.as_bytes().get(cursor) != Some(&b'>') {
        return Err(ScanError::Malformed(
            "the XML document has a malformed closing tag",
        ));
    }
    Ok((name, cursor + 1))
}

fn read_qualified_name(text: &str, from: usize) -> Result<&str, ScanError> {
    let bytes = text.as_bytes();
    let mut cursor = from;
    let mut colon = false;
    while let Some(byte) = bytes.get(cursor).copied() {
        let first = cursor == from || bytes.get(cursor.wrapping_sub(1)) == Some(&b':');
        let valid = if first {
            byte.is_ascii_alphabetic() || byte == b'_'
        } else {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
        };
        if valid {
            cursor += 1;
        } else if byte == b':' && !colon && cursor > from {
            colon = true;
            cursor += 1;
        } else {
            break;
        }
    }
    if cursor == from || bytes.get(cursor.wrapping_sub(1)) == Some(&b':') {
        return Err(ScanError::Malformed(
            "the XML document has an invalid XML name",
        ));
    }
    if let Some(next) = bytes.get(cursor)
        && (*next == b':'
            || (!is_xml_whitespace(*next) && !matches!(*next, b'/' | b'=' | b'>' | b'<' | b'?')))
    {
        return Err(ScanError::Malformed(
            "the XML document has an invalid qualified XML name",
        ));
    }
    Ok(&text[from..cursor])
}

fn apply_namespaces(
    parent: Option<&HashMap<String, String>>,
    declarations: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut namespaces = parent.cloned().unwrap_or_default();
    for (prefix, namespace) in declarations {
        namespaces.insert(prefix.clone(), namespace.clone());
    }
    namespaces
}

fn namespace_for<'a>(name: &str, namespaces: &'a HashMap<String, String>) -> &'a str {
    if prefix_of(name) == Some("xml") {
        return XML_NAMESPACE;
    }
    namespaces
        .get(prefix_of(name).unwrap_or(""))
        .map_or("", String::as_str)
}

fn prefix_of(name: &str) -> Option<&str> {
    name.split_once(':').map(|(prefix, _)| prefix)
}

fn local_name(name: &str) -> &str {
    name.split_once(':').map_or(name, |(_, local)| local)
}

fn decode_namespace_value(value: &str) -> Result<String, ScanError> {
    let mut decoded = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative) = value[cursor..].find('&') {
        let start = cursor + relative;
        decoded.push_str(&value[cursor..start]);
        let end = value[start..]
            .find(';')
            .map(|offset| start + offset)
            .ok_or(ScanError::Malformed(
                "the XML document has a malformed entity reference",
            ))?;
        let body = &value[start + 1..end];
        match body {
            "amp" => decoded.push('&'),
            "lt" => decoded.push('<'),
            "gt" => decoded.push('>'),
            "quot" => decoded.push('"'),
            "apos" => decoded.push('\''),
            _ => decoded.push(decode_numeric_entity(body)?),
        }
        cursor = end + 1;
    }
    decoded.push_str(&value[cursor..]);
    Ok(decoded)
}

fn decode_numeric_entity(body: &str) -> Result<char, ScanError> {
    let (digits, radix) = body
        .strip_prefix("#x")
        .map(|digits| (digits, 16))
        .or_else(|| body.strip_prefix('#').map(|digits| (digits, 10)))
        .ok_or(ScanError::Malformed(
            "the XML document has an invalid attribute entity",
        ))?;
    if digits.is_empty()
        || !digits.bytes().all(|byte| {
            if radix == 16 {
                byte.is_ascii_hexdigit()
            } else {
                byte.is_ascii_digit()
            }
        })
    {
        return Err(ScanError::Malformed(
            "the XML document has an invalid attribute entity",
        ));
    }
    let point = u32::from_str_radix(digits, radix)
        .ok()
        .and_then(char::from_u32)
        .filter(|character| is_xml_character(*character))
        .ok_or(ScanError::Malformed(
            "the XML document has an invalid attribute entity",
        ))?;
    Ok(point)
}

fn required_terminator(text: &str, from: usize, terminator: &str) -> Result<usize, ScanError> {
    text[from..]
        .find(terminator)
        .map(|offset| from + offset + terminator.len())
        .ok_or(ScanError::Malformed(
            "the XML document has an unterminated structure",
        ))
}

/// Finds the end of a comment so an injection cannot land inside one.
///
/// An unterminated comment is refused because there is then no trustworthy insertion point. What
/// the body contains is not checked: a stray `--` makes the document the browser's problem, not
/// a reason to refuse to serve it.
fn comment_end(text: &str, opening: usize) -> Result<usize, ScanError> {
    required_terminator(text, opening + 4, "-->")
}

fn processing_instruction_end(text: &str, opening: usize) -> Result<usize, ScanError> {
    let name = read_qualified_name(text, opening + 2)?;
    let name_end = opening + 2 + name.len();
    if !matches!(text.as_bytes().get(name_end), Some(b'?'))
        && !text
            .as_bytes()
            .get(name_end)
            .is_some_and(|byte| is_xml_whitespace(*byte))
    {
        return Err(ScanError::Malformed(
            "the XML document has a malformed processing instruction",
        ));
    }
    let end = required_terminator(text, name_end, "?>")?;
    // A CSS or page-template stylesheet leaves the document this scanner is reading, so it is
    // none of its business. An XSLT stylesheet replaces the document the browser builds, which
    // would discard the injection and leave a frame with no agent in it, so that one is refused
    // while it can still be reported.
    if name.eq_ignore_ascii_case("xml-stylesheet") && declares_xslt(&text[name_end..end]) {
        return Err(ScanError::Malformed(
            "an XSLT stylesheet would replace the document before the agent could run",
        ));
    }
    Ok(end)
}

/// Skips the doctype declaration, whatever it declares.
///
/// Browsers never fetch external identifiers, and an internal subset is how an older XHTML
/// publication declares the named entities it then uses, so neither is this scanner's business:
/// its only need is to know where the declaration ends so an injection does not land inside it.
/// Quoted literals and the internal subset are tracked for exactly that reason. Whether the
/// declaration is well formed, and whether its name matches the root, is for the XML parser that
/// reads this document next.
/// Whether an `xml-stylesheet` instruction names an XSLT transform.
///
/// A declared XSLT type settles it. Absent a type, an `.xsl` or `.xslt` href is treated as one
/// too: guessing wrong in that direction reports a document that cannot work, where guessing the
/// other way serves a frame that silently never starts.
fn declares_xslt(pseudo_attributes: &str) -> bool {
    let lowered = pseudo_attributes.to_ascii_lowercase();
    if lowered.contains("text/xsl") || lowered.contains("application/xslt+xml") {
        return true;
    }
    !lowered.contains("type=")
        && lowered
            .split(['"', '\''])
            .any(|value| value.ends_with(".xsl") || value.ends_with(".xslt"))
}

fn doctype_end(text: &str, opening: usize) -> Result<usize, ScanError> {
    let bytes = text.as_bytes();
    let unterminated =
        || ScanError::Malformed("the XML document has an unterminated doctype declaration");
    let mut cursor = opening + "<!DOCTYPE".len();
    if !bytes
        .get(cursor)
        .is_some_and(|byte| is_xml_whitespace(*byte) || *byte == b'>')
    {
        return Err(ScanError::Malformed(
            "the XML document has an unsupported declaration",
        ));
    }
    while cursor < text.len() {
        match bytes[cursor] {
            b'>' => return Ok(cursor + 1),
            quote @ (b'"' | b'\'') => {
                let body = cursor + 1;
                cursor = text[body..]
                    .find(quote as char)
                    .map(|offset| body + offset + 1)
                    .ok_or_else(unterminated)?;
            }
            b'[' => {
                // An internal subset may itself contain quoted literals holding `]`.
                cursor += 1;
                while cursor < text.len() {
                    match bytes[cursor] {
                        b']' => break,
                        quote @ (b'"' | b'\'') => {
                            let body = cursor + 1;
                            cursor = text[body..]
                                .find(quote as char)
                                .map(|offset| body + offset + 1)
                                .ok_or_else(unterminated)?;
                        }
                        _ => cursor += 1,
                    }
                }
                if cursor >= text.len() {
                    return Err(unterminated());
                }
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }
    Err(unterminated())
}

fn skip_whitespace(text: &str, mut cursor: usize) -> usize {
    while text
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| is_xml_whitespace(*byte))
    {
        cursor += 1;
    }
    cursor
}

fn is_xml_whitespace(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | b'\r' | b' ')
}

fn encode_xhtml_injection(
    options: XhtmlInstrumentationOptions<'_>,
) -> Result<Vec<u8>, InstrumentationError> {
    let invalid = InstrumentationError;
    let mut value = String::new();
    if let Some(url) = options.reader_font_stylesheet_url {
        value.push_str(&format!(
            "<link xmlns=\"{XHTML_NAMESPACE}\" rel=\"stylesheet\" href=\"{}\"{} data-epub-navigator-reader-font=\"reader-serif\" />",
            escape_xml_attribute(url).map_err(invalid)?,
            crossorigin_attribute(options.crossorigin),
        ));
    }
    if let Some(url) = options.auxiliary_script_url {
        append_external_script(
            &mut value,
            XHTML_NAMESPACE,
            "src",
            url,
            None,
            options.crossorigin,
            None,
        )
        .map_err(invalid)?;
    }
    append_external_script(
        &mut value,
        XHTML_NAMESPACE,
        "src",
        options.content_agent_url,
        options.nonce,
        options.crossorigin,
        options.nested_publication_base_url,
    )
    .map_err(invalid)?;
    Ok(value.into_bytes())
}

fn encode_svg_injection(
    options: SvgInstrumentationOptions<'_>,
) -> Result<Vec<u8>, InstrumentationError> {
    let invalid = InstrumentationError;
    let mut value = String::new();
    if let Some(url) = options.auxiliary_script_url {
        append_external_script(
            &mut value,
            SVG_NAMESPACE,
            "href",
            url,
            None,
            options.crossorigin,
            None,
        )
        .map_err(invalid)?;
    }
    append_external_script(
        &mut value,
        SVG_NAMESPACE,
        "href",
        options.content_agent_url,
        options.nonce,
        options.crossorigin,
        options.nested_publication_base_url,
    )
    .map_err(invalid)?;
    Ok(value.into_bytes())
}

fn append_external_script(
    output: &mut String,
    namespace: &str,
    url_attribute: &str,
    url: &str,
    nonce: Option<&str>,
    crossorigin: bool,
    nested_publication_base_url: Option<&str>,
) -> Result<(), &'static str> {
    output.push_str(&format!(
        "<script xmlns=\"{namespace}\" {url_attribute}=\"{}\"{}{}{}></script>",
        escape_xml_attribute(url)?,
        nonce
            .map(escape_xml_attribute)
            .transpose()?
            .map_or_else(String::new, |nonce| format!(" nonce=\"{nonce}\"")),
        crossorigin_attribute(crossorigin),
        nested_publication_base_url
            .map(escape_xml_attribute)
            .transpose()?
            .map_or_else(String::new, |url| format!(
                " data-epub-navigator-nested-publication-base-url=\"{url}\""
            )),
    ));
    Ok(())
}

fn crossorigin_attribute(enabled: bool) -> &'static str {
    if enabled {
        " crossorigin=\"anonymous\""
    } else {
        ""
    }
}

fn escape_xml_attribute(value: &str) -> Result<String, &'static str> {
    if !value.chars().all(is_xml_character) {
        return Err("an injected XML attribute contains an invalid XML character");
    }
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        escaped.push_str(match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&apos;",
            '\t' => "&#x9;",
            '\n' => "&#xA;",
            '\r' => "&#xD;",
            _ => {
                escaped.push(character);
                continue;
            }
        });
    }
    Ok(escaped)
}

fn is_xml_character(character: char) -> bool {
    matches!(character as u32, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xhtml(source: &[u8]) -> Result<Vec<u8>, InstrumentationError> {
        instrument_xhtml(
            source,
            XhtmlInstrumentationOptions {
                content_agent_url: "/agent.js",
                nested_publication_base_url: None,
                reader_font_stylesheet_url: None,
                nonce: None,
                auxiliary_script_url: None,
                crossorigin: false,
            },
        )
    }

    fn svg(source: &[u8]) -> Result<Vec<u8>, InstrumentationError> {
        instrument_svg(
            source,
            SvgInstrumentationOptions {
                content_agent_url: "/agent.js",
                nested_publication_base_url: None,
                nonce: None,
                auxiliary_script_url: None,
                crossorigin: false,
            },
        )
    }

    #[test]
    fn xhtml_locates_safe_namespaced_head_insertions() {
        for document in [
            format!(
                "<html xmlns=\"{XHTML_NAMESPACE}\"><head profile=\"value\"><title>x</title></head></html>"
            ),
            format!(
                "<x:html xmlns:x=\"{XHTML_NAMESPACE}\"><x:head><x:title>x</x:title></x:head></x:html>"
            ),
            format!(
                "<x:html xmlns:x=\"{XHTML_NAMESPACE}\"><h:head xmlns:h=\"{XHTML_NAMESPACE}\"></h:head></x:html>"
            ),
            format!(
                "<html xmlns=\"{XHTML_NAMESPACE}\"><!-- <head> --><head data-x=\"a > b\"></head></html>"
            ),
            format!(
                "<!DOCTYPE html><html xmlns=\"{XHTML_NAMESPACE}\"><![CDATA[<head>]]><head></head></html>"
            ),
            format!("<html xmlns=\"{XHTML_NAMESPACE}\"><?probe <head> ?><head></head></html>"),
        ] {
            let transformed = String::from_utf8(xhtml(document.as_bytes()).unwrap()).unwrap();
            let injection =
                format!("<script xmlns=\"{XHTML_NAMESPACE}\" src=\"/agent.js\"></script>");
            assert!(transformed.contains(&injection));
            assert!(transformed.find(&injection) < transformed.rfind("head>"));
        }
    }

    #[test]
    fn xhtml_ignores_foreign_and_nested_heads() {
        let document = format!(
            "<x:html xmlns:x=\"{XHTML_NAMESPACE}\" xmlns:f=\"urn:foreign\"><f:section xmlns:x=\"urn:nested\"><x:head /></f:section><x:head></x:head></x:html>"
        );
        let transformed = String::from_utf8(xhtml(document.as_bytes()).unwrap()).unwrap();
        assert!(transformed.find("<x:head />").unwrap() < transformed.find("/agent.js").unwrap());
        assert!(
            xhtml(
                format!("<html xmlns=\"{XHTML_NAMESPACE}\"><body><head></head></body></html>")
                    .as_bytes()
            )
            .unwrap_err()
            .to_string()
            .contains("no direct head")
        );
    }

    #[test]
    fn scanners_stop_after_the_safe_insertion_point() {
        let xhtml_source = format!(
            "<html xmlns=\"{XHTML_NAMESPACE}\"><head><script>publisher()</script><body title=\"&publisher;\">\0"
        );
        let svg_source = format!(
            "<svg xmlns=\"{SVG_NAMESPACE}\"><script>publisher()</script><g title=\"&publisher;\">\0"
        );

        assert!(
            String::from_utf8(xhtml(xhtml_source.as_bytes()).unwrap())
                .unwrap()
                .contains("/agent.js")
        );
        assert!(
            String::from_utf8(svg(svg_source.as_bytes()).unwrap())
                .unwrap()
                .contains("/agent.js")
        );
    }

    #[test]
    fn xhtml_preserves_bom_and_authored_bytes_and_orders_escaped_injections() {
        let before = format!(
            "<?xml version='1.0'?><x:html xmlns:x='{XHTML_NAMESPACE}'><x:head data-title='café &amp; tea'>"
        );
        let after = "\r\n<x:title>東京</x:title></x:head><x:body>naïve</x:body></x:html>";
        let mut source = UTF8_BOM.to_vec();
        source.extend_from_slice(before.as_bytes());
        source.extend_from_slice(after.as_bytes());
        let authored = source.clone();
        let transformed = instrument_xhtml(
            &source,
            XhtmlInstrumentationOptions {
                content_agent_url: "/agent?one=1&two=\"2\"",
                nested_publication_base_url: Some("https://nested.test/?a=1&b=2"),
                reader_font_stylesheet_url: Some("/font?<>&\"'"),
                nonce: Some("nonce<&>\"'\n"),
                auxiliary_script_url: Some("/probe?a=1&b=2"),
                crossorigin: false,
            },
        )
        .unwrap();
        assert_eq!(
            &transformed[..UTF8_BOM.len() + before.len()],
            &authored[..UTF8_BOM.len() + before.len()]
        );
        assert_eq!(
            &transformed[transformed.len() - after.len()..],
            after.as_bytes()
        );
        let text = String::from_utf8(transformed).unwrap();
        assert!(text.contains("href=\"/font?&lt;&gt;&amp;&quot;&apos;\""));
        assert!(text.contains("src=\"/agent?one=1&amp;two=&quot;2&quot;\" nonce=\"nonce&lt;&amp;&gt;&quot;&apos;&#xA;\""));
        assert!(text.contains(
            "data-epub-navigator-nested-publication-base-url=\"https://nested.test/?a=1&amp;b=2\""
        ));
        assert!(text.find("/font?").unwrap() < text.find("/probe?").unwrap());
        assert!(text.find("/probe?").unwrap() < text.find("/agent?").unwrap());
        assert_eq!(source, authored);
    }

    #[test]
    fn svg_accepts_external_doctype_identifiers_unchanged() {
        let doctype = "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">";
        let source =
            format!("{doctype}<svg xmlns=\"http://www.w3.org/2000/svg\"><title>x</title></svg>");
        let transformed = String::from_utf8(svg(source.as_bytes()).unwrap()).unwrap();
        assert!(transformed.starts_with(doctype));
    }

    #[test]
    fn xhtml_accepts_external_doctype_identifiers_unchanged() {
        for doctype in [
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">",
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\"\n  \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\">",
            "<!DOCTYPE html PUBLIC '-//W3C//DTD XHTML 1.0 Transitional//EN' 'http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd' >",
            "<!DOCTYPE html SYSTEM \"about:legacy-compat\">",
        ] {
            let source = format!(
                "{doctype}<html xmlns=\"{XHTML_NAMESPACE}\"><head></head><body>a&nbsp;b</body></html>"
            );
            let transformed = String::from_utf8(xhtml(source.as_bytes()).unwrap()).unwrap();
            assert!(transformed.starts_with(doctype), "{doctype}");
            assert!(transformed.contains("a&nbsp;b"));
        }
    }

    #[test]
    fn xhtml_rejects_what_would_make_the_injection_unsafe() {
        // Each of these either has no trustworthy insertion point, or would let a publisher
        // script run before the agent. Nothing here is a judgement about well-formedness.
        let vectors = [
            // The root is not `html` in the XHTML namespace, so this is not a document whose
            // head can be identified.
            "<html><head></head></html>",
            "<html xmlns=\"urn:not-xhtml\"><head></head></html>",
            "<body xmlns=\"http://www.w3.org/1999/xhtml\"></body>",
            "<x:html><x:head></x:head></x:html>",
            "<xml:html xmlns:xml=\"http://www.w3.org/1999/xhtml\"><xml:head></xml:head></xml:html>",
            "<?probe value?><!-- comment -->",
            // Nowhere to insert a child.
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" />",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head /></html>",
            // No direct `head` child: XML is case sensitive, and a `body` is not a head.
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><HEAD></HEAD></html>",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body /></html>",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head xmlns=\"urn:foreign\"></head></html>",
            // A publisher script would run before the agent.
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><script></script><head></head></html>",
            // An unterminated structure leaves no offset that is known to be outside it.
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><!-- comment<head></head></html>",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><![CDATA[x<head></head></html>",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head title=\"value></head></html>",
            "<!DOCTYPE html [<!ENTITY probe \"x\"><html xmlns=\"http://www.w3.org/1999/xhtml\"><head/></html>",
            // An XSLT stylesheet would replace the document and discard the injection.
            "<?xml-stylesheet type=\"text/xsl\" href=\"t.xsl\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><head></head></html>",
            "<?xml-stylesheet href=\"transform.xsl\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><head></head></html>",
        ];
        for document in vectors {
            assert!(xhtml(document.as_bytes()).is_err(), "{document}");
        }
        assert_eq!(
            xhtml(&[0xff]).unwrap_err().to_string(),
            "the XHTML document is not valid UTF-8"
        );
    }

    /// What the browser's own XML parser decides, this scanner serves.
    ///
    /// Refusing these produced a 422 and a broken frame for documents a browser either renders or
    /// reports itself. The internal entity subset and the non-XSLT stylesheet instruction are the
    /// two that real publications actually use.
    #[test]
    fn xhtml_serves_what_the_xml_parser_adjudicates() {
        let head = "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head></head></html>";
        let vectors = [
            // Named entities in an older publication come from an internal subset.
            format!("<!DOCTYPE html [<!ENTITY nbsp \"&#160;\">]>{head}"),
            format!("<!DOCTYPE html PUBLIC \"id\" \"about:blank\" [<!ENTITY probe \"x\">]>{head}"),
            // An Adobe page template, which is how many older EPUBs carry layout.
            format!(
                "<?xml-stylesheet type=\"application/vnd.adobe-page-template+xml\" href=\"page-template.xpgt\"?>{head}"
            ),
            format!("<?xml-stylesheet type=\"text/css\" href=\"style.css\"?>{head}"),
            // Doctype shapes this scanner used to adjudicate and no longer does.
            format!("<!DOCTYPE html PUBLIC \"id\">{head}"),
            format!("<!DOCTYPE html PUBLIC id \"about:blank\">{head}"),
            format!("<!DOCTYPE html SYSTEM \"a<b\">{head}"),
            format!("<!DOCTYPE htmlPUBLIC \"id\" \"about:blank\">{head}"),
            format!("<!DOCTYPE svg>{head}"),
            format!("<!DOCTYPE HTML>{head}"),
            // Character data and comment bodies the parser will accept or report for itself.
            "<html xmlns=\"http://www.w3.org/1999/xhtml\">&publisher;<head></head></html>"
                .to_owned(),
            "<html xmlns=\"http://www.w3.org/1999/xhtml\">]]><head></head></html>".to_owned(),
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><!-- one -- two --><head></head></html>"
                .to_owned(),
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head title=\"&#0;\"></head></html>"
                .to_owned(),
            format!("text{head}"),
        ];
        for document in vectors {
            let instrumented =
                xhtml(document.as_bytes()).unwrap_or_else(|error| panic!("{document}: {error}"));
            let instrumented = String::from_utf8(instrumented).unwrap();
            assert!(instrumented.contains("/agent.js"), "{document}");
            // The authored bytes are returned unchanged around the injection.
            assert!(instrumented.len() > document.len(), "{document}");
        }
    }

    #[test]
    fn xhtml_rejects_invalid_injections() {
        let source = format!("<html xmlns=\"{XHTML_NAMESPACE}\"><head></head></html>");
        for (agent, probe) in [("/agent\0.js", None), ("/agent.js", Some("/probe\0.js"))] {
            let error = instrument_xhtml(
                source.as_bytes(),
                XhtmlInstrumentationOptions {
                    content_agent_url: agent,
                    nested_publication_base_url: None,
                    reader_font_stylesheet_url: None,
                    nonce: None,
                    auxiliary_script_url: probe,
                    crossorigin: false,
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains("invalid XML character"));
        }
    }

    #[test]
    fn svg_inserts_first_child_for_prefixed_and_complex_documents() {
        let source = format!(
            "<?xml version=\"1.0\"?><!-- <svg/> --><!DOCTYPE svg><svg xmlns=\"{SVG_NAMESPACE}\"><![CDATA[<svg/>]]><title>x</title></svg>"
        );
        let transformed = String::from_utf8(svg(source.as_bytes()).unwrap()).unwrap();
        assert!(transformed.contains(&format!("<svg xmlns=\"{SVG_NAMESPACE}\"><script xmlns=\"{SVG_NAMESPACE}\" href=\"/agent.js\"></script><![CDATA[")));
        let prefixed = format!("<s:svg xmlns:s=\"{SVG_NAMESPACE}\"></s:svg>");
        assert!(
            String::from_utf8(svg(prefixed.as_bytes()).unwrap())
                .unwrap()
                .contains("<s:svg xmlns:s=")
        );
    }

    #[test]
    fn svg_preserves_source_and_escapes_ordered_explicit_injections() {
        let before =
            format!("<?xml version='1.0'?><svg  xmlns='{SVG_NAMESPACE}' data-x='a&amp;b'>");
        let after = "\r\n<!-- authored --><path d='M0 0'/></svg>";
        let mut source = UTF8_BOM.to_vec();
        source.extend_from_slice(before.as_bytes());
        source.extend_from_slice(after.as_bytes());
        let transformed = instrument_svg(
            &source,
            SvgInstrumentationOptions {
                content_agent_url: "/agent?one=1&two=\"2\"<'",
                nested_publication_base_url: Some("https://nested.test/?a=1&b=2"),
                nonce: Some("nonce<&>\"'\n"),
                auxiliary_script_url: Some("/probe.js"),
                crossorigin: false,
            },
        )
        .unwrap();
        assert_eq!(
            &transformed[..UTF8_BOM.len() + before.len()],
            &source[..UTF8_BOM.len() + before.len()]
        );
        assert_eq!(
            &transformed[transformed.len() - after.len()..],
            after.as_bytes()
        );
        let text = String::from_utf8(transformed).unwrap();
        assert!(text.find("/probe.js").unwrap() < text.find("/agent?").unwrap());
        assert!(text.contains("href=\"/agent?one=1&amp;two=&quot;2&quot;&lt;&apos;\""));
        assert!(text.contains("nonce=\"nonce&lt;&amp;&gt;&quot;&apos;&#xA;\""));
    }

    #[test]
    fn svg_rejects_what_would_make_the_injection_unsafe() {
        let vectors = [
            "<svg xmlns=\"urn:not-svg\"></svg>",
            "<s:svg></s:svg>",
            "<?xml version=\"1.0\"?><!-- only -->",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            "<x a=\"<svg xmlns='http://www.w3.org/2000/svg'>\"></x>",
            "<!DOCTYPE svg [<!ENTITY probe \"x\"><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
            "<?xml-stylesheet type=\"text/xsl\" href=\"t.xsl\"?><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
            "<?xml-stylesheet href=\"transform.xsl\"?><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        ];
        for document in vectors {
            assert!(svg(document.as_bytes()).is_err(), "{document}");
        }
        assert_eq!(
            svg(&[b'<', b's', b'v', b'g', 0xff])
                .unwrap_err()
                .to_string(),
            "the SVG document is not valid UTF-8"
        );
    }

    #[test]
    fn svg_serves_what_the_xml_parser_adjudicates() {
        let root = "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        let vectors = [
            format!("<!DOCTYPE svg [<!ENTITY probe \"x\">]>{root}"),
            format!("<!DOCTYPE svg SYSTEM \"about:blank\" [<!ENTITY probe \"x\">]>{root}"),
            format!("<!DOCTYPE svg PUBLIC \"id\">{root}"),
            format!("<!DOCTYPE html>{root}"),
            format!("<!DOCTYPE SVG>{root}"),
            format!("<?xml-stylesheet type=\"text/css\" href=\"style.css\"?>{root}"),
            "<svg xmlns=\"http://www.w3.org/2000/svg\" title=\"&publisher;\"></svg>".to_owned(),
        ];
        for document in vectors {
            let instrumented =
                svg(document.as_bytes()).unwrap_or_else(|error| panic!("{document}: {error}"));
            assert!(
                String::from_utf8(instrumented)
                    .unwrap()
                    .contains("/agent.js"),
                "{document}"
            );
        }
    }
}
