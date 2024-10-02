use crate::{
    compiler,
    error::{error, no_error, ErrorIterator, ValidationError},
    keywords::CompilationResult,
    paths::JsonPointerNode,
    primitive_type::PrimitiveType,
    validator::Validate,
};
use ahash::AHashMap;
use once_cell::sync::Lazy;
use serde_json::{Map, Value};

use crate::paths::JsonPointer;
use std::{collections::VecDeque, ops::Index, sync::Mutex};

// Use regex::Regex here to take advantage of replace_all method not available in fancy_regex::Regex
static CONTROL_GROUPS_RE: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"\\c[A-Za-z]").expect("Is a valid regex"));

static REGEX_CACHE: Lazy<Mutex<LruCache>> = Lazy::new(|| Mutex::new(LruCache::new(10)));

struct LruCache {
    map: AHashMap<String, fancy_regex::Regex>,
    queue: VecDeque<String>,
    capacity: usize,
}

impl LruCache {
    fn new(capacity: usize) -> Self {
        LruCache {
            map: AHashMap::new(),
            queue: VecDeque::new(),
            capacity,
        }
    }

    fn get(&mut self, key: &str) -> Option<&fancy_regex::Regex> {
        if let Some(value) = self.map.get(key) {
            let index = self.queue.iter().position(|x| x == key).unwrap();
            let k = self.queue.remove(index).unwrap();
            self.queue.push_back(k);
            Some(value)
        } else {
            None
        }
    }

    fn insert(&mut self, key: String, value: fancy_regex::Regex) -> Option<fancy_regex::Regex> {
        if self.map.len() >= self.capacity && !self.map.contains_key(&key) {
            if let Some(lru_key) = self.queue.pop_front() {
                self.map.remove(&lru_key);
            }
        }

        let old_value = self.map.insert(key.clone(), value);
        if old_value.is_some() {
            let index = self.queue.iter().position(|x| x == &key).unwrap();
            self.queue.remove(index);
        }
        self.queue.push_back(key);
        old_value
    }
}

pub(crate) struct PatternValidator {
    original: String,
    pattern: fancy_regex::Regex,
    schema_path: JsonPointer,
}

impl PatternValidator {
    #[inline]
    pub(crate) fn compile<'a>(
        ctx: &compiler::Context,
        pattern: &'a Value,
    ) -> CompilationResult<'a> {
        match pattern {
            Value::String(item) => {
                let mut cache = REGEX_CACHE.lock().expect("Lock is poisoned");
                let pattern = if let Some(regex) = cache.get(item) {
                    regex.clone()
                } else {
                    let regex = match convert_regex(item) {
                        Ok(r) => r,
                        Err(_) => {
                            return Err(ValidationError::format(
                                JsonPointer::default(),
                                ctx.clone().into_pointer(),
                                pattern,
                                "regex",
                            ))
                        }
                    };
                    cache.insert(item.clone(), regex.clone());
                    regex
                };
                Ok(Box::new(PatternValidator {
                    original: item.clone(),
                    pattern,
                    schema_path: ctx.as_pointer_with("pattern"),
                }))
            }
            _ => Err(ValidationError::single_type_error(
                JsonPointer::default(),
                ctx.clone().into_pointer(),
                pattern,
                PrimitiveType::String,
            )),
        }
    }
}

impl Validate for PatternValidator {
    fn validate<'instance>(
        &self,
        instance: &'instance Value,
        instance_path: &JsonPointerNode,
    ) -> ErrorIterator<'instance> {
        if let Value::String(item) = instance {
            match self.pattern.is_match(item) {
                Ok(is_match) => {
                    if !is_match {
                        return error(ValidationError::pattern(
                            self.schema_path.clone(),
                            instance_path.into(),
                            instance,
                            self.original.clone(),
                        ));
                    }
                }
                Err(e) => {
                    return error(ValidationError::backtrack_limit(
                        self.schema_path.clone(),
                        instance_path.into(),
                        instance,
                        e,
                    ));
                }
            }
        }
        no_error()
    }

    fn is_valid(&self, instance: &Value) -> bool {
        if let Value::String(item) = instance {
            return self.pattern.is_match(item).unwrap_or(false);
        }
        true
    }
}

// ECMA 262 has differences
#[allow(clippy::result_large_err)]
pub(crate) fn convert_regex(pattern: &str) -> Result<fancy_regex::Regex, fancy_regex::Error> {
    // replace control chars
    let new_pattern = CONTROL_GROUPS_RE.replace_all(pattern, replace_control_group);
    let mut out = String::with_capacity(new_pattern.len());
    let mut chars = new_pattern.chars().peekable();
    // To convert character group we need to iterate over chars and in case of `\` take a look
    // at the next char to detect whether this group should be converted
    while let Some(current) = chars.next() {
        if current == '\\' {
            // Possible character group
            if let Some(next) = chars.next() {
                match next {
                    'd' => out.push_str("[0-9]"),
                    'D' => out.push_str("[^0-9]"),
                    'w' => out.push_str("[A-Za-z0-9_]"),
                    'W' => out.push_str("[^A-Za-z0-9_]"),
                    's' => {
                        out.push_str("[ \t\n\r\u{000b}\u{000c}\u{2003}\u{feff}\u{2029}\u{00a0}]")
                    }
                    'S' => {
                        out.push_str("[^ \t\n\r\u{000b}\u{000c}\u{2003}\u{feff}\u{2029}\u{00a0}]")
                    }
                    _ => {
                        // Nothing interesting, push as is
                        out.push(current);
                        out.push(next)
                    }
                }
            } else {
                // End of the string, push the last char.
                // Note that it is an incomplete escape sequence and will lead to an error on
                // the next step
                out.push(current);
            }
        } else {
            // Regular character
            out.push(current);
        }
    }
    fancy_regex::Regex::new(&out)
}

#[allow(clippy::arithmetic_side_effects)]
fn replace_control_group(captures: &regex::Captures) -> String {
    // There will be no overflow, because the minimum value is 65 (char 'A')
    ((captures
        .index(0)
        .trim_start_matches(r"\c")
        .chars()
        .next()
        .expect("This is always present because of the regex rule. It has [A-Za-z] next")
        .to_ascii_uppercase() as u8
        - 64) as char)
        .to_string()
}

#[inline]
pub(crate) fn compile<'a>(
    ctx: &compiler::Context,
    _: &'a Map<String, Value>,
    schema: &'a Value,
) -> Option<CompilationResult<'a>> {
    Some(PatternValidator::compile(ctx, schema))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_util;
    use serde_json::json;
    use test_case::test_case;

    #[test_case(r"^[\w\-\.\+]+$", "CC-BY-4.0", true)]
    #[test_case(r"^[\w\-\.\+]+$", "CC-BY-!", false)]
    #[test_case(r"^\W+$", "1_0", false)]
    #[test_case(r"\\w", r"\w", true)]
    fn regex_matches(pattern: &str, text: &str, is_matching: bool) {
        let validator = convert_regex(pattern).expect("A valid regex");
        assert_eq!(
            validator.is_match(text).expect("A valid pattern"),
            is_matching
        );
    }

    #[test_case(r"\")]
    #[test_case(r"\d\")]
    fn invalid_escape_sequences(pattern: &str) {
        assert!(convert_regex(pattern).is_err())
    }

    #[test_case("^(?!eo:)", "eo:bands", false)]
    #[test_case("^(?!eo:)", "proj:epsg", true)]
    fn negative_lookbehind_match(pattern: &str, text: &str, is_matching: bool) {
        let text = json!(text);
        let schema = json!({"pattern": pattern});
        let validator = crate::validator_for(&schema).unwrap();
        assert_eq!(validator.is_valid(&text), is_matching)
    }

    #[test]
    fn schema_path() {
        tests_util::assert_schema_path(&json!({"pattern": "^f"}), &json!("b"), "/pattern")
    }
}
