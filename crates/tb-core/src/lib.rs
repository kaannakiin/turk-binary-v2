#[must_use]
pub fn greeting() -> &'static str {
    "merhaba"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_is_turkish() {
        assert_eq!(greeting(), "merhaba");
    }
}
