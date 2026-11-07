//! Pages and sources by URI. A long text comes in slices, each naming the next, as resources have no cursors of their own.
use crate::respond;
use memex_api::{SourceDto, VaultDto};
use memex_core::{Caller, Server, ServerEdges, ServerError};
use memex_types::{ObjectId, VaultId};
use rmcp::model::{ListResourceTemplatesResult, ReadResourceResult, Resource, ResourceContents, ResourceTemplate};
use serde_json::{Value, json};

const VAULTS_URI: &str = "memex://vaults";

pub(crate) fn page_uri(vault: &VaultId, id: &str) -> String {
    format!("memex://vaults/{}/pages/{id}", vault.0)
}

fn source_uri(vault: &VaultId, id: &str) -> String {
    format!("memex://vaults/{}/sources/{id}", vault.0)
}

/// What can be read, listed the same for everyone: static, like the tools.
pub(crate) fn templates() -> ListResourceTemplatesResult {
    ListResourceTemplatesResult::with_all_items(vec![
        ResourceTemplate::new("memex://vaults/{vault}/pages/{id}{?offset}", "page").with_description("A wiki page's Markdown."),
        ResourceTemplate::new("memex://vaults/{vault}/sources/{id}{?offset}", "source").with_description("A source's current extracted text."),
    ])
}

pub(crate) fn fixed() -> Vec<Resource> {
    vec![
        Resource::new(VAULTS_URI, "vaults")
            .with_description("The vaults this token sees.")
            .with_mime_type("application/json"),
    ]
}

enum Target {
    Vaults,
    Page(VaultId, ObjectId, usize),
    Source(VaultId, ObjectId, usize),
}

fn parse_uri(uri: &str) -> Option<Target> {
    if uri == VAULTS_URI {
        return Some(Target::Vaults);
    }
    let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
    let rest = path.strip_prefix("memex://vaults/")?;
    let offset = match query {
        "" => 0,
        q => q.strip_prefix("offset=")?.parse().ok()?,
    };
    let mut parts = rest.split('/');
    let (vault, kind, id) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || vault.is_empty() || id.is_empty() {
        return None;
    }
    let (vault, id) = (VaultId(vault.into()), ObjectId(id.into()));
    match kind {
        "pages" => Some(Target::Page(vault, id, offset)),
        "sources" => Some(Target::Source(vault, id, offset)),
        _ => None,
    }
}

/// The slice of `text` from `offset`, ending on a character boundary, and where the next one starts.
fn slice(text: &str, offset: usize, room: usize) -> Result<(&str, Option<usize>), ServerError> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err(ServerError::Invalid("that offset isn't one this resource gave out".into()));
    }
    let mut end = (offset + room).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok((&text[offset..end], (end < text.len()).then_some(end)))
}

/// The contents: facts about the text as data with a trust tier, then the slice itself.
fn with_text(uri: &str, base: &str, mut facts: Value, mime: &str, text: &str, offset: usize, max: usize) -> Result<ReadResourceResult, ServerError> {
    let tiers = respond::trust_tiers(&facts);
    // JSON around the facts, and escapes in the text, don't count toward what a slice holds, so it stays well inside the limit.
    let (part, next) = slice(text, offset, (max / 2).max(1))?;
    if let Some(map) = facts.as_object_mut() {
        map.insert("total_bytes".into(), text.len().into());
        map.insert("offset".into(), offset.into());
        if let Some(n) = next {
            map.insert("next".into(), format!("{base}?offset={n}").into());
        }
    }
    let facts = ResourceContents::text(respond::envelope(tiers, next.is_some(), facts), uri).with_mime_type("application/json");
    Ok(ReadResourceResult::new(vec![facts, ResourceContents::text(part, uri).with_mime_type(mime)]))
}

/// Reads one resource. None means no resource looks like that.
pub(crate) fn read<E>(s: &Server<E>, c: &Caller, uri: &str, max: usize) -> Result<Option<ReadResourceResult>, ServerError>
where
    E: ServerEdges,
{
    let Some(target) = parse_uri(uri) else {
        return Ok(None);
    };
    Ok(Some(match target {
        Target::Vaults => {
            let rows = s.list_vaults(c, None, 200)?;
            let items: Vec<VaultDto> = rows.into_iter().map(Into::into).collect();
            let text = respond::envelope(vec![], false, json!({ "items": items }));
            ReadResourceResult::new(vec![ResourceContents::text(text, uri).with_mime_type("application/json")])
        }
        Target::Page(vault, id, offset) => {
            let view = memex_api::page_detail(s.get_wiki_page(c, &vault, &id, None)?);
            let facts = json!({"id": view.id, "title": view.title, "slug": view.slug, "revision": view.revision});
            with_text(uri, &page_uri(&vault, &id.0), facts, "text/markdown", &view.body, offset, max)?
        }
        Target::Source(vault, id, offset) => {
            let row: SourceDto = s.get_source(c, &vault, &id)?.into();
            let renditions = s.list_renditions(c, &vault, &id)?;
            let current = renditions.iter().rfind(|r| r.superseded_by.is_none());
            let facts = serde_json::to_value(&row).unwrap_or(Value::Null);
            let base = source_uri(&vault, &id.0);
            match current {
                Some(r) => {
                    let bytes = s.rendition_blob(c, &vault, &r.header.id, false)?;
                    with_text(uri, &base, facts, "text/plain", &String::from_utf8_lossy(&bytes), offset, max)?
                }
                // Not processed yet, so there's no text. The job's progress says when.
                None => with_text(uri, &base, facts, "text/plain", "", 0, max)?,
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_parse_or_are_refused() {
        assert!(matches!(parse_uri("memex://vaults"), Some(Target::Vaults)));
        assert!(matches!(parse_uri("memex://vaults/v/pages/p"), Some(Target::Page(_, _, 0))));
        assert!(matches!(parse_uri("memex://vaults/v/sources/s?offset=9"), Some(Target::Source(_, _, 9))));
        for bad in [
            "memex://vaults/v/pages",
            "memex://vaults/v/other/x",
            "memex://vaults/v/pages/p/extra",
            "memex://vaults/v/pages/p?offset=x",
            "memex://vaults/v/pages/p?other=1",
            "http://evil/",
            "",
        ] {
            assert!(parse_uri(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn slices_end_on_character_boundaries_and_cover_the_text() {
        let text = "åäö".repeat(50);
        let (mut out, mut at) = (String::new(), Some(0));
        while let Some(start) = at {
            let (part, next) = slice(&text, start, 7).unwrap();
            assert!(!part.is_empty());
            out.push_str(part);
            at = next;
        }
        assert_eq!(out, text);
        assert!(slice(&text, 1, 7).is_err(), "inside a character");
        assert!(slice(&text, text.len() + 1, 7).is_err());
    }
}
