//! Deserialisers that accept a 64-bit id as either a JSON number or a string.
//!
//! JavaScript carries 53 bits of integer precision, so a browser client cannot
//! round-trip a Snowflake id as a number — `350710414700445696` comes back out
//! as `350710414700445700`. The admin therefore parses ids as strings and
//! sends them back the same way.
//!
//! Accepting both keeps the API compatible with existing clients (Rust, Go,
//! curl) that quite reasonably send a number, while letting browsers send the
//! exact value.

use serde::de::{Deserializer, Error as DeError, Unexpected};
use serde::Deserialize;

/// A single id as it may arrive on the wire.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawId {
    Number(i64),
    Text(String),
}

impl RawId {
    fn into_i64<E: DeError>(self) -> Result<i64, E> {
        match self {
            Self::Number(n) => Ok(n),
            Self::Text(s) => s
                .parse::<i64>()
                .map_err(|_| E::invalid_value(Unexpected::Str(&s), &"a 64-bit integer id")),
        }
    }
}

/// `Option<i64>` from a number, a string, or null.
///
/// # Errors
/// Fails when a string is present but is not a valid integer.
pub fn option<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<RawId>::deserialize(deserializer)?;
    raw.map(RawId::into_i64).transpose()
}

/// `Option<Option<i64>>` for three-state fields, where an explicit `null`
/// means "clear this" and an absent key means "leave it alone".
///
/// The nested `Option` is the point here rather than an oversight: JSON
/// genuinely distinguishes an absent key from a null one, and a re-parent
/// endpoint needs both.
///
/// # Errors
/// Fails when a string is present but is not a valid integer.
#[allow(clippy::option_option)]
pub fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<RawId>::deserialize(deserializer)?;
    Ok(Some(raw.map(RawId::into_i64).transpose()?))
}

/// `Vec<i64>` whose elements may be numbers or strings.
///
/// # Errors
/// Fails when an element is a string that is not a valid integer.
pub fn vec<'de, D>(deserializer: D) -> Result<Vec<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Vec::<RawId>::deserialize(deserializer)?;
    raw.into_iter().map(RawId::into_i64).collect()
}

/// `Option<Vec<i64>>` whose elements may be numbers or strings.
///
/// # Errors
/// Fails when an element is a string that is not a valid integer.
pub fn option_vec<'de, D>(deserializer: D) -> Result<Option<Vec<i64>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<Vec<RawId>>::deserialize(deserializer)?;
    raw.map(|items| items.into_iter().map(RawId::into_i64).collect())
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Body {
        #[serde(default, deserialize_with = "option")]
        parent_id: Option<i64>,
        #[serde(default, deserialize_with = "option_vec")]
        term_ids: Option<Vec<i64>>,
    }

    #[test]
    fn accepts_numbers_and_strings_alike() {
        let from_number: Body =
            serde_json::from_str(r#"{"parent_id":350710414700445696,"term_ids":[1,2]}"#)
                .expect("number form");
        assert_eq!(from_number.parent_id, Some(350_710_414_700_445_696));
        assert_eq!(from_number.term_ids, Some(vec![1, 2]));

        // The browser sends the exact digits as a string.
        let from_string: Body =
            serde_json::from_str(r#"{"parent_id":"350710414700445696","term_ids":["1","2"]}"#)
                .expect("string form");
        assert_eq!(from_string.parent_id, Some(350_710_414_700_445_696));
        assert_eq!(from_string.term_ids, Some(vec![1, 2]));
    }

    #[test]
    fn absent_and_null_stay_none() {
        let empty: Body = serde_json::from_str("{}").expect("empty");
        assert_eq!(empty.parent_id, None);
        assert_eq!(empty.term_ids, None);

        let nulls: Body =
            serde_json::from_str(r#"{"parent_id":null,"term_ids":null}"#).expect("nulls");
        assert_eq!(nulls.parent_id, None);
        assert_eq!(nulls.term_ids, None);
    }

    #[test]
    fn rejects_a_string_that_is_not_an_id() {
        let err = serde_json::from_str::<Body>(r#"{"parent_id":"not-a-number"}"#);
        assert!(err.is_err(), "non-numeric string must be rejected");
    }

    #[test]
    fn three_state_distinguishes_absent_from_null() {
        #[derive(Deserialize)]
        struct Tri {
            #[serde(default, deserialize_with = "double_option")]
            #[allow(clippy::option_option)]
            parent_id: Option<Option<i64>>,
        }

        let absent: Tri = serde_json::from_str("{}").expect("absent");
        assert_eq!(absent.parent_id, None, "absent means leave alone");

        let cleared: Tri = serde_json::from_str(r#"{"parent_id":null}"#).expect("null");
        assert_eq!(cleared.parent_id, Some(None), "null means clear");

        let set: Tri = serde_json::from_str(r#"{"parent_id":"42"}"#).expect("set");
        assert_eq!(set.parent_id, Some(Some(42)));
    }
}
