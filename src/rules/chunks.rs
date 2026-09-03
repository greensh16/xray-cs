//! Literal chunk-spec analysis, shared by XR012 and DK010.
//!
//! Array *shapes* are unknowable without a runtime, so xray says nothing about
//! whether a chunking is well proportioned. Chunk *arguments*, though, are
//! right there in the source, and the two worst chunkings are recognisable
//! from the literal alone:
//!
//!   * a chunk length of `1` along a dimension — one task, and on Lustre one
//!     metadata round-trip, per index along that axis;
//!   * a chunk whose literal extents multiply out to a handful of elements —
//!     the scheduler spends more time dispatching the task than the task
//!     spends computing.
//!
//! Everything here works on literals only. A chunk spec built from variables
//! (`chunks={"time": step}`) reports [`ChunkVerdict::Unknown`], and every
//! caller must treat that as "say nothing" — the same `None`-is-not-safe
//! invariant the binding tracker follows.

use tree_sitter::Node;

use crate::parser::node_text;

/// A chunk whose literal extents multiply to fewer than this many elements is
/// too small to be worth a task.
///
/// Dask's own guidance is chunks of at least ~1 MB — around 130k float64
/// elements. This threshold sits three orders of magnitude below that on
/// purpose: it is not "suboptimal chunking", it is a chunk spec that can only
/// have been a mistake, so it stays quiet on merely debatable choices.
pub const TRIVIAL_CHUNK_ELEMENTS: i64 = 64;

/// What a literal chunk specification turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkVerdict {
    /// A chunk length of exactly 1, along the named dimension when the spec
    /// was a dict (`chunks={"time": 1}` → `Some("time")`), or positionally.
    SingletonChunk { dim: Option<String> },
    /// Every extent was an integer literal and they multiply to a very small
    /// number of elements.
    TriviallySmall { elements: i64 },
    /// Nothing conclusive: variables, `"auto"`, `-1`, a sensible size, or a
    /// shape this analysis does not model. Never report on this.
    Unknown,
}

/// Inspect a chunk specification node — the value of `chunks=`, or the first
/// positional argument of `.chunk()` / `.rechunk()`.
///
/// Dict forms are checked per entry so the offending dimension can be named.
/// Tuple and list forms are positional and therefore cover every dimension,
/// which is what makes their product meaningful; a dict's does not, since the
/// dimensions it omits keep their full extent.
pub fn classify_chunk_spec(node: Node<'_>, source: &[u8]) -> ChunkVerdict {
    match node.kind() {
        "dictionary" => classify_dict(node, source),
        "tuple" | "list" => classify_sequence(node, source),
        // `.rechunk(1)` / `chunks=1` applies that length to every dimension —
        // the most extreme spelling of the mistake.
        "integer" => {
            if literal_int(node, source) == Some(1) {
                ChunkVerdict::SingletonChunk { dim: None }
            } else {
                ChunkVerdict::Unknown
            }
        }
        _ => ChunkVerdict::Unknown,
    }
}

/// A dict spec names the dimensions it constrains and leaves every other one
/// at full extent, so `{"time": 24}` on a (time, lat, lon) dataset is a chunk
/// of 24 × 181 × 360, not 24. That makes the small-product test meaningless
/// here — only the singleton verdict, which is per-dimension, survives.
fn classify_dict(node: Node<'_>, source: &[u8]) -> ChunkVerdict {
    let mut cursor = node.walk();

    for pair in node.named_children(&mut cursor) {
        if pair.kind() != "pair" {
            continue;
        }
        let Some(value) = pair.child_by_field_name("value") else {
            continue;
        };
        if literal_int(value, source) == Some(1) {
            let dim = pair
                .child_by_field_name("key")
                .map(|k| strip_quotes(node_text(&k, source)).to_string());
            return ChunkVerdict::SingletonChunk { dim };
        }
    }

    ChunkVerdict::Unknown
}

fn classify_sequence(node: Node<'_>, source: &[u8]) -> ChunkVerdict {
    let mut cursor = node.walk();
    let mut extents: Vec<i64> = Vec::new();
    let mut all_literal = true;

    for element in node.named_children(&mut cursor) {
        match literal_int(element, source) {
            Some(1) => return ChunkVerdict::SingletonChunk { dim: None },
            Some(n) => extents.push(n),
            // A per-dimension chunk *sequence* — `chunks=((10, 10), (5,))` —
            // and every non-literal extent land here.
            None => all_literal = false,
        }
    }

    trivial_product(&extents, all_literal)
}

/// The trivially-small verdict, but only when every extent was a literal:
/// a single unknown extent could be enormous, so the product proves nothing.
fn trivial_product(extents: &[i64], all_literal: bool) -> ChunkVerdict {
    if !all_literal || extents.is_empty() {
        return ChunkVerdict::Unknown;
    }
    let product = extents.iter().try_fold(1i64, |acc, n| acc.checked_mul(*n));
    match product {
        Some(p) if p < TRIVIAL_CHUNK_ELEMENTS => ChunkVerdict::TriviallySmall { elements: p },
        _ => ChunkVerdict::Unknown,
    }
}

/// The value of a positive integer literal, if that is what `node` is.
///
/// `-1` (dask and xarray's "one chunk along this axis") parses as a
/// `unary_operator` and so returns `None`, which is right: it is a deliberate
/// instruction, not an accidental singleton.
fn literal_int(node: Node<'_>, source: &[u8]) -> Option<i64> {
    if node.kind() != "integer" {
        return None;
    }
    node_text(&node, source)
        .replace('_', "")
        .parse::<i64>()
        .ok()
}

fn strip_quotes(text: &str) -> &str {
    text.trim_matches(|c| c == '"' || c == '\'')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_source;
    use tree_sitter::Node;

    /// Classify the value of the single `chunks=` keyword in `expr`.
    fn verdict(expr: &str) -> ChunkVerdict {
        let src = format!("f({expr})\n");
        let parsed = parse_source(src).unwrap();
        let source = parsed.source.as_bytes();
        let node = find_kwarg_value(parsed.tree.root_node(), source).expect("chunks= kwarg");
        classify_chunk_spec(node, source)
    }

    fn find_kwarg_value<'t>(node: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
        if node.kind() == "keyword_argument"
            && node
                .child_by_field_name("name")
                .is_some_and(|n| node_text(&n, source) == "chunks")
        {
            return node.child_by_field_name("value");
        }
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .find_map(|c| find_kwarg_value(c, source))
    }

    #[test]
    fn a_dict_entry_of_one_names_its_dimension() {
        assert_eq!(
            verdict("chunks={\"time\": 1, \"lat\": 180}"),
            ChunkVerdict::SingletonChunk {
                dim: Some("time".to_string())
            }
        );
    }

    #[test]
    fn tuples_and_bare_integers_report_a_singleton_without_a_name() {
        assert_eq!(
            verdict("chunks=(1, 180, 360)"),
            ChunkVerdict::SingletonChunk { dim: None }
        );
        assert_eq!(
            verdict("chunks=1"),
            ChunkVerdict::SingletonChunk { dim: None }
        );
    }

    #[test]
    fn a_tiny_literal_product_is_trivially_small() {
        assert_eq!(
            verdict("chunks=(2, 4, 4)"),
            ChunkVerdict::TriviallySmall { elements: 32 }
        );
    }

    #[test]
    fn a_partial_dict_is_never_trivially_small() {
        // `{"time": 24}` leaves lat and lon whole, so the chunk is 24 × 181 ×
        // 360 elements, not 24. Reading the product off a dict was a false
        // positive on the single most ordinary chunk spec there is.
        assert_eq!(verdict("chunks={\"time\": 24}"), ChunkVerdict::Unknown);
        assert_eq!(
            verdict("chunks={\"x\": 2, \"y\": 4}"),
            ChunkVerdict::Unknown
        );
    }

    #[test]
    fn sensible_and_non_literal_specs_say_nothing() {
        assert_eq!(
            verdict("chunks={\"time\": 24, \"lat\": 181}"),
            ChunkVerdict::Unknown
        );
        assert_eq!(verdict("chunks=\"auto\""), ChunkVerdict::Unknown);
        assert_eq!(verdict("chunks=(step, 180)"), ChunkVerdict::Unknown);
        // -1 means "one chunk along this axis" — deliberate, not a mistake.
        assert_eq!(verdict("chunks={\"time\": -1}"), ChunkVerdict::Unknown);
    }

    #[test]
    fn one_unknown_extent_blocks_the_small_product_verdict() {
        // (2, n) could be (2, 10_000_000). Only a singleton is provable here.
        assert_eq!(verdict("chunks=(2, n)"), ChunkVerdict::Unknown);
    }
}
