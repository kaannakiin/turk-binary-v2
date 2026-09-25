/// Set bit positions of a little-endian multi-word bitmap (word 0 holds bits
/// 0..64), the layout of the `uint`/`ruint` wide integers the programs use.
pub(crate) fn set_bits(words: &[u64]) -> impl Iterator<Item = i64> + '_ {
    words.iter().zip(0i64..).flat_map(|(&word, index)| {
        (0..64i64)
            .filter(move |bit| (word >> bit) & 1 == 1)
            .map(move |bit| index * 64 + bit)
    })
}

#[cfg(test)]
mod tests {
    use super::set_bits;

    #[test]
    fn bits_are_numbered_from_the_low_bit_of_word_zero() {
        let bits: Vec<i64> = set_bits(&[0b101, 0, 1 << 63]).collect();
        assert_eq!(bits, [0, 2, 191]);
    }
}
