pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_positive_numbers() {
        assert_eq!(add(2, 3), 5);
    }

    #[test]
    fn adds_negative_numbers() {
        assert_eq!(add(-2, -3), -5);
    }

    #[test]
    fn deliberately_fails() {
        assert_eq!(add(1, 1), 3, "fixture regression: 1 + 1 must stay 2");
    }
}
