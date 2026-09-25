use domain::Pubkey;

pub(crate) fn read_pubkey(data: &[u8], offset: usize) -> Option<Pubkey> {
    Some(Pubkey::new_from_array(read_array(data, offset)?))
}

pub(crate) fn read_bool(data: &[u8], offset: usize) -> Option<bool> {
    match data.get(offset)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

pub(crate) fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    read_array(data, offset).map(u16::from_le_bytes)
}

pub(crate) fn read_u64_words<const N: usize>(data: &[u8], offset: usize) -> Option<[u64; N]> {
    let mut words = [0u64; N];
    for (i, word) in words.iter_mut().enumerate() {
        *word = u64::from_le_bytes(read_array(data, offset.checked_add(i.checked_mul(8)?)?)?);
    }
    Some(words)
}

fn read_array<const N: usize>(data: &[u8], offset: usize) -> Option<[u8; N]> {
    data.get(offset..offset.checked_add(N)?)?.try_into().ok()
}
