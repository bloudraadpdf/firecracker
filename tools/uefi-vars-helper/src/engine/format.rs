// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
use der::asn1::ObjectIdentifier;

pub type Guid = [u8; 16];
pub type Status = Result<(), u64>;
pub const INVALID: u64 = 0x8000_0000_0000_0002;
pub const UNSUPPORTED: u64 = 0x8000_0000_0000_0003;
pub const BAD_SIZE: u64 = 0x8000_0000_0000_0004;
pub const TOO_SMALL: u64 = 0x8000_0000_0000_0005;
pub const PROTECTED: u64 = 0x8000_0000_0000_0008;
pub const NO_SPACE: u64 = 0x8000_0000_0000_0009;
pub const NOT_FOUND: u64 = 0x8000_0000_0000_000e;
pub const ALREADY: u64 = 0x8000_0000_0000_0014;
pub const SECURITY: u64 = 0x8000_0000_0000_001a;
pub const STORAGE: usize = 1024 * 1024;
pub const VARIABLE_OVERHEAD: usize = 64;
pub const MAX_FRAME: usize = 2 * 1024 * 1024;
pub const NV: u32 = 1;
pub const BS: u32 = 2;
pub const RT: u32 = 4;
pub const AUTH: u32 = 0x20;
pub const APPEND: u32 = 0x40;
pub const SIG_DB_ATTRIBUTES: u32 = NV | BS | RT | AUTH;

pub const fn guid(value: u128) -> Guid {
    let b = value.to_be_bytes();
    [
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15],
    ]
}
pub const GLOBAL: Guid = guid(0x8be4df61_93ca_11d2_aa0d_00e098032b8c);
pub const VENDOR_KEYS: Guid = guid(0x9073e4e0_60ec_4b6e_9903_4c223c260f3c);
pub const IMAGE: Guid = guid(0xd719b2cb_3d3a_4596_a3bc_dad00e67656f);
pub const CUSTOM: Guid = guid(0xc076ec0c_7028_4399_a072_71ee5c448b9f);
pub const ENABLE: Guid = guid(0xf0a30bc7_af08_4556_99c4_001009c93a44);
pub const X509: Guid = guid(0xa5c059a1_94e4_4aa7_87b5_ab155c2bf072);
pub const HASH256: Guid = guid(0xc1c41626_504c_4092_aca9_41f936934328);
pub const HASH384: Guid = guid(0xff3e5307_9fd0_48c9_85f1_8ad56c701e01);
pub const HASH512: Guid = guid(0x093e0fae_a6c4_4f50_9f1b_d41e2b89c19a);
pub const RSA2048: Guid = guid(0x3c5766e8_269c_4e34_aa14_ed776e85b3b6);
pub const PKCS7: Guid = guid(0x4aafd29d_68df_49ee_8aa9_347d375665a7);
pub const VARIABLE_PROTOCOL: Guid = guid(0xed32d533_99e6_4209_9cc0_2d72cdd998a7);
pub const POLICY_PROTOCOL: Guid = guid(0xda1b0d11_d1a7_46c4_9dc9_f3714875c6eb);
pub const END_DXE: Guid = guid(0x02ce967a_dd7e_4ffc_9ee7_810cf0470880);
pub const READY: Guid = guid(0x7ce88fb3_4bd7_4679_87a8_a8d8dee50d2b);
pub const EXIT: Guid = guid(0x27abf055_b1b8_4c26_8048_748f37baa2df);
pub const SHA256_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
pub const RSA_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
pub const RSA_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
pub const SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
pub const DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");

pub fn name(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}
pub fn valid_name(bytes: &[u8], empty: bool) -> bool {
    let (units, rest) = bytes.as_chunks::<2>();
    rest.is_empty()
        && !units.is_empty()
        && units.last() == Some(&[0, 0])
        && (empty || units.len() > 1)
        && !units[..units.len() - 1].contains(&[0, 0])
        && char::decode_utf16(
            units[..units.len() - 1]
                .iter()
                .map(|u| u16::from_le_bytes(*u)),
        )
        .all(|c| c.is_ok())
}
pub fn number<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], u64> {
    bytes
        .get(offset..offset.checked_add(N).ok_or(BAD_SIZE)?)
        .ok_or(BAD_SIZE)?
        .try_into()
        .map_err(|_| BAD_SIZE)
}
pub fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, u64> {
    Ok(u16::from_le_bytes(number(bytes, offset)?))
}
pub fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, u64> {
    Ok(u32::from_le_bytes(number(bytes, offset)?))
}
pub fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, u64> {
    Ok(u64::from_le_bytes(number(bytes, offset)?))
}
pub fn size_at(bytes: &[u8], offset: usize) -> Result<usize, u64> {
    usize::try_from(u64_at(bytes, offset)?).map_err(|_| BAD_SIZE)
}
pub fn put32(bytes: &mut [u8], offset: usize, value: usize) {
    bytes[offset..offset + 4].copy_from_slice(&u32::try_from(value).unwrap().to_le_bytes());
}
pub fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
pub fn bounded_range(bytes: &[u8], start: usize, size: usize) -> Result<&[u8], u64> {
    bytes
        .get(start..start.checked_add(size).ok_or(BAD_SIZE)?)
        .ok_or(BAD_SIZE)
}
