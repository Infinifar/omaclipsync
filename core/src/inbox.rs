//! Only explicit, ID-checked requests may read the sensitive body.
use rustclipsync_core::state::InboxItem;
use serde_json::{json, Value};

pub const CHUNK_BYTES: usize = 16 * 1024;

pub fn metadata(item: Option<&InboxItem>) -> Value {
    item.map(|item| json!({"id": item.id, "receivedAt": item.received_at}))
        .unwrap_or(Value::Null)
}

pub fn read_chunk(item: &InboxItem, id: &str, offset: u64) -> Result<Value, &'static str> {
    let offset = usize::try_from(offset).map_err(|_| "invalid_request")?;
    if item.id != id {
        return Err("inbox_stale");
    }
    if offset > item.text.len() || !item.text.is_char_boundary(offset) {
        return Err("invalid_request");
    }
    let mut end = (offset + CHUNK_BYTES).min(item.text.len());
    while !item.text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(
        json!({"id": item.id, "receivedAt": item.received_at, "offset": offset,
        "nextOffset": end, "total": item.text.len(), "text": &item.text[offset..end]}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_excludes_body_and_explicit_reads_are_bounded_and_unicode_safe() {
        let item = InboxItem {
            id: rustclipsync_core::state::random_uuid().unwrap(),
            text: format!(
                "{}😀中文<img src=\"https://invalid.test\">",
                "\0\"\\".repeat(6000)
            ),
            received_at: 42,
        };
        let meta = metadata(Some(&item));
        assert!(meta.get("text").is_none());
        assert_eq!(metadata(None), Value::Null);
        let mut offset = 0;
        let mut text = String::new();
        while offset < item.text.len() as u64 {
            let chunk = read_chunk(&item, &item.id, offset).unwrap();
            assert!(serde_json::to_vec(&chunk).unwrap().len() < 128 * 1024);
            text.push_str(chunk["text"].as_str().unwrap());
            offset = chunk["nextOffset"].as_u64().unwrap();
        }
        assert_eq!(text, item.text);
        assert_eq!(read_chunk(&item, "old", 0), Err("inbox_stale"));
        assert_eq!(read_chunk(&item, &item.id, 18001), Err("invalid_request"));
        assert!(read_chunk(&item, &item.id, u64::MAX).is_err());
    }
}
