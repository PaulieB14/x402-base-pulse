//! ABI decoders for x402 protocol events on Base
//!
//! Detects x402 settlements through two EVM event patterns:
//!
//! 1. **EIP-3009 (primary)**: `AuthorizationUsed(address indexed authorizer, bytes32 indexed nonce)`
//!    emitted by USDC when `transferWithAuthorization` is called. Per the x402 protocol docs
//!    (https://docs.cdp.coinbase.com/x402), facilitators settle payments this way.
//!
//! 2. **Permit2 proxy (secondary)**: `Settled()` and `SettledWithPermit()` events from the
//!    x402ExactPermit2Proxy contract (parameterless events, currently testnet only).
//!
//! Also decodes ERC-20 `Transfer` events to extract payment amounts.

use substreams::Hex;
use substreams_ethereum::pb::eth::v2::Log;

// =============================================
// Event topic hashes (keccak256)
// Computed via: cast keccak "EventSignature(params)"
// =============================================

/// Transfer(address indexed from, address indexed to, uint256 value)
pub const TRANSFER_TOPIC: [u8; 32] = [
    0xdd, 0xf2, 0x52, 0xad, 0x1b, 0xe2, 0xc8, 0x9b,
    0x69, 0xc2, 0xb0, 0x68, 0xfc, 0x37, 0x8d, 0xaa,
    0x95, 0x2b, 0xa7, 0xf1, 0x63, 0xc4, 0xa1, 0x16,
    0x28, 0xf5, 0x5a, 0x4d, 0xf5, 0x23, 0xb3, 0xef,
];

/// AuthorizationUsed(address indexed authorizer, bytes32 indexed nonce)
/// keccak256("AuthorizationUsed(address,bytes32)")
pub const AUTHORIZATION_USED_TOPIC: [u8; 32] = [
    0x98, 0xde, 0x50, 0x35, 0x28, 0xee, 0x59, 0xb5,
    0x75, 0xef, 0x0c, 0x0a, 0x25, 0x76, 0xa8, 0x24,
    0x97, 0xbf, 0xc0, 0x29, 0xa5, 0x68, 0x5b, 0x20,
    0x9e, 0x9e, 0xc3, 0x33, 0x47, 0x9b, 0x10, 0xa5,
];

/// Settled() - x402 proxy event (no parameters)
/// keccak256("Settled()")
pub const SETTLED_TOPIC: [u8; 32] = [
    0x97, 0x08, 0x8e, 0xc3, 0x60, 0x6c, 0xfe, 0x8c,
    0xc1, 0x12, 0x18, 0x05, 0x70, 0xd0, 0x3f, 0xcd,
    0xe0, 0x5f, 0x9b, 0x8e, 0x1b, 0xfe, 0xf8, 0xe2,
    0x77, 0x84, 0xea, 0xf5, 0xdd, 0x56, 0x91, 0xb6,
];

/// SettledWithPermit() - x402 proxy event (no parameters)
/// keccak256("SettledWithPermit()")
pub const SETTLED_WITH_PERMIT_TOPIC: [u8; 32] = [
    0xde, 0x5b, 0x89, 0xd1, 0x0f, 0xc8, 0x00, 0xc4,
    0x59, 0x32, 0x9c, 0x38, 0x2f, 0xab, 0xfc, 0xad,
    0x0b, 0xe0, 0xed, 0x7e, 0x53, 0x28, 0xe0, 0x1f,
    0xae, 0x04, 0xe5, 0x07, 0xb0, 0x9e, 0xf5, 0xd8,
];

// =============================================
// Decoded event structs
// =============================================

/// Decoded ERC-20 Transfer event
pub struct TransferEvent {
    pub from: Vec<u8>,
    pub to: Vec<u8>,
    pub amount: String,
    pub log_index: u32,
}

/// Decoded EIP-3009 AuthorizationUsed event
pub struct AuthorizationUsedEvent {
    pub authorizer: Vec<u8>,
    pub nonce: Vec<u8>,
    pub log_index: u32,
}

// =============================================
// Decoders
// =============================================

/// Decode ERC-20 Transfer event
/// Event: Transfer(address indexed from, address indexed to, uint256 value)
pub fn decode_erc20_transfer(log: &Log) -> Option<TransferEvent> {
    if log.topics.len() < 3 || log.data.len() < 32 {
        return None;
    }
    if log.topics[0] != TRANSFER_TOPIC {
        return None;
    }

    let from = log.topics[1][12..32].to_vec();
    let to = log.topics[2][12..32].to_vec();
    let amount = parse_uint256(&log.data[0..32]);

    Some(TransferEvent {
        from,
        to,
        amount,
        log_index: log.index,
    })
}

/// Decode EIP-3009 AuthorizationUsed event
/// Event: AuthorizationUsed(address indexed authorizer, bytes32 indexed nonce)
///
/// Emitted by USDC when transferWithAuthorization is called.
/// The authorizer is the payer who signed the EIP-3009 authorization.
pub fn decode_authorization_used(log: &Log) -> Option<AuthorizationUsedEvent> {
    if log.topics.len() < 3 {
        return None;
    }
    if log.topics[0] != AUTHORIZATION_USED_TOPIC {
        return None;
    }

    let authorizer = log.topics[1][12..32].to_vec();
    let nonce = log.topics[2].clone();

    Some(AuthorizationUsedEvent {
        authorizer,
        nonce,
        log_index: log.index,
    })
}

/// Check if a log is a Settled() event from the x402 proxy
pub fn is_settled_event(log: &Log) -> bool {
    !log.topics.is_empty() && log.topics[0] == SETTLED_TOPIC
}

/// Check if a log is a SettledWithPermit() event from the x402 proxy
pub fn is_settled_with_permit_event(log: &Log) -> bool {
    !log.topics.is_empty() && log.topics[0] == SETTLED_WITH_PERMIT_TOPIC
}

/// Parse uint256 from 32-byte big-endian slice
pub fn parse_uint256(data: &[u8]) -> String {
    if data.len() != 32 {
        return "0".to_string();
    }
    let result = num_bigint::BigUint::from_bytes_be(data);
    result.to_string()
}

/// Format raw bytes as a 0x-prefixed hex address
pub fn format_address(bytes: &[u8]) -> String {
    format!("0x{}", Hex(bytes).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_uint256_zero() {
        let data = [0u8; 32];
        assert_eq!(parse_uint256(&data), "0");
    }

    #[test]
    fn test_parse_uint256_one() {
        let mut data = [0u8; 32];
        data[31] = 1;
        assert_eq!(parse_uint256(&data), "1");
    }

    #[test]
    fn test_parse_uint256_usdc_amount() {
        // 1,000,000 = 1 USDC (6 decimals)
        let mut data = [0u8; 32];
        data[29] = 0x0F;
        data[30] = 0x42;
        data[31] = 0x40;
        assert_eq!(parse_uint256(&data), "1000000");
    }

    #[test]
    fn test_format_address() {
        let bytes = [0xAB; 20];
        let addr = format_address(&bytes);
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 42);
    }
}
