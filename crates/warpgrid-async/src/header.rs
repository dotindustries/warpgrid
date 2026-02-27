/// An HTTP header as a name-value pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// An ordered collection of HTTP headers.
///
/// Preserves insertion order and supports duplicate header names
/// (e.g., multiple `Set-Cookie` headers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderMap {
    entries: Vec<Header>,
}

impl HeaderMap {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.entries.push(Header::new(name, value));
    }

    /// Get the first header value matching `name` (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    /// Get all header values matching `name` (case-insensitive).
    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Header> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn into_vec(self) -> Vec<Header> {
        self.entries
    }
}

impl FromIterator<Header> for HeaderMap {
    fn from_iter<I: IntoIterator<Item = Header>>(iter: I) -> Self {
        Self {
            entries: iter.into_iter().collect(),
        }
    }
}

impl FromIterator<(String, String)> for HeaderMap {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Self {
            entries: iter
                .into_iter()
                .map(|(n, v)| Header::new(n, v))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_new() {
        let h = Header::new("Content-Type", "application/json");
        assert_eq!(h.name, "Content-Type");
        assert_eq!(h.value, "application/json");
    }

    #[test]
    fn header_map_insert_and_get() {
        let mut map = HeaderMap::new();
        map.insert("Content-Type", "text/html");
        assert_eq!(map.get("content-type"), Some("text/html"));
        assert_eq!(map.get("Content-Type"), Some("text/html"));
    }

    #[test]
    fn header_map_get_missing() {
        let map = HeaderMap::new();
        assert_eq!(map.get("X-Missing"), None);
    }

    #[test]
    fn header_map_duplicate_headers() {
        let mut map = HeaderMap::new();
        map.insert("Set-Cookie", "a=1");
        map.insert("Set-Cookie", "b=2");

        assert_eq!(map.get("Set-Cookie"), Some("a=1"));
        assert_eq!(map.get_all("Set-Cookie"), vec!["a=1", "b=2"]);
    }

    #[test]
    fn header_map_len_and_empty() {
        let mut map = HeaderMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        map.insert("X-Test", "1");
        assert!(!map.is_empty());
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn header_map_from_iterator() {
        let map: HeaderMap = vec![
            ("Host".to_string(), "example.com".to_string()),
            ("Accept".to_string(), "*/*".to_string()),
        ]
        .into_iter()
        .collect();

        assert_eq!(map.len(), 2);
        assert_eq!(map.get("host"), Some("example.com"));
    }

    #[test]
    fn header_map_into_vec() {
        let mut map = HeaderMap::new();
        map.insert("A", "1");
        map.insert("B", "2");

        let vec = map.into_vec();
        assert_eq!(vec.len(), 2);
        assert_eq!(vec[0].name, "A");
        assert_eq!(vec[1].name, "B");
    }
}
