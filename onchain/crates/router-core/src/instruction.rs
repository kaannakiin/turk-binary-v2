pub struct HopMeta {
    pub key: [u8; 32],
    pub is_signer: bool,
    pub is_writable: bool,
}

pub struct HopInstruction {
    pub program_id: [u8; 32],
    pub metas: Vec<HopMeta>,
    pub data: Vec<u8>,
}

pub struct BuiltHop {
    pub ix: HopInstruction,
    pub in_ata_index: usize,
    pub out_ata_index: usize,
}
