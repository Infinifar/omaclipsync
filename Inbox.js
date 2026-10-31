// Bounded, explicit-only preview state. Discard this state on hide or identity change.
var previewLimit = 64 * 1024

function empty() {
  return {id: "", text: "", offset: 0, loading: false, revealed: false, truncated: false}
}

function begin(id) {
  return {id: id, text: "", offset: 0, loading: true, revealed: false, truncated: false}
}

function utf8Prefix(text, limit) {
  var bytes = 0, end = 0
  while (end < text.length) {
    var code = text.charCodeAt(end), length = 1, size
    if (code >= 0xd800 && code <= 0xdbff) {
      var low = text.charCodeAt(end + 1)
      if (!(low >= 0xdc00 && low <= 0xdfff)) return null
      size = 4; length = 2
    } else if (code >= 0xdc00 && code <= 0xdfff) return null
    else size = code < 0x80 ? 1 : code < 0x800 ? 2 : 3
    if (bytes + size > limit) break
    bytes += size; end += length
  }
  return {text: text.slice(0, end), bytes: bytes}
}

function append(state, chunk) {
  if (!state.loading || !chunk || chunk.id !== state.id || typeof chunk.text !== "string" ||
      chunk.offset !== state.offset || !Number.isInteger(chunk.nextOffset) ||
      !Number.isInteger(chunk.total) || chunk.total < 0 || chunk.total > 10 * 1024 * 1024 ||
      chunk.nextOffset < chunk.offset || chunk.nextOffset > chunk.total ||
      (chunk.nextOffset === chunk.offset && chunk.nextOffset !== chunk.total)) return null
  var full = utf8Prefix(chunk.text, 16 * 1024)
  if (!full || full.text !== chunk.text || full.bytes !== chunk.nextOffset - chunk.offset) return null
  var part = utf8Prefix(chunk.text, previewLimit - state.offset)
  if (!part) return null
  var loading = chunk.nextOffset < chunk.total && chunk.nextOffset < previewLimit && part.bytes === full.bytes
  return {id: state.id, text: state.text + part.text, offset: chunk.nextOffset,
    loading: loading, revealed: !loading,
    truncated: !loading && (chunk.nextOffset < chunk.total || part.bytes < full.bytes)}
}
