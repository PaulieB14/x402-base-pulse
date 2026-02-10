//! ABI decoders for x402 protocol events on Base
//!
//! Handles decoding of:
//! - x402ExactPermit2Proxy events (Settled, SettledWithPermit)
//! - ERC-20 Transfer events (USDC payment correlation)
//! - EIP-3009 AuthorizationUsed events

use substreams::Hex;
use substreams_ethereum::pb::eth::v2::Log;

/// Decoded ERC-20 Transfer event
pub struct TransferEvent {
    pub from: Vec<u8>,
    pub to: Vec<u8>,
    pub amount: String,
}

/// Decoded x402 proxy settlement event
pub struct ProxySettlementEvent {
    /// "settled" or "settled_with_permit"
    pub settlement_type: String,
    /// First topic hash (event signature)
    pub event_sig: String,
    /// Raw hex-encoded event data for future decoding
    pub raw_data: String,
    /// Decoded payer if extractable from event data
    pub payer: Option<Vec<u8>>,
    /// Decoded recipient if extractable from event data
    pub recipient: Option<Vec<u8>>,
    /// Decoded token address if extractable from event data
    pub token: Option<Vec<u8>>,
    /// Decoded amount if extractable from event data
    pub amount: Option<String>,
}

// ERC-20 Transfer(address indexed from, address indexed to, uint256 value)
const TRANSFER_SIG: [u8; 32] = [
    0xdd, 0xf2, 0x52, 0xad, 0x1b, 0xe2, 0xc8, 0x9b,
    0x69, 0xc2, 0xb0, 0x68, 0xfc, 0x37, 0x8d, 0xaa,
    0x95, 0x2b, 0xa7, 0xf1, 0x63, 0xc4, 0xa1, 0x16,
    0x28, 0xf5, 0x5a, 0x4d, 0xf5, 0x23, 0xb3, 0xef,
];

/// Decode ERC-20 Transfer event
/// Event: Transfer(address indexed from, address indexed to, uint256 value)
pub fn decode_erc20_transfer(log: &Log) -> Option<TransferEvent> {
    if log.topics.len() < 3 || log.data.len() < 32 {
        return None;
    }

    if log.topics[0] != TRANSFER_SIG {
        return None;
    }

    let from = log.topics[1][12..32].to_vec();
    let to = log.topics[2][12..32].to_vec();
    let amount = parse_uint256(&log.data[0..32]);

    Some(TransferEvent { from, to, amount })
}

/// Attempt to decode a settlement event from the x402 proxy contract.
///
/// Since the exact event ABI is not published in the x402 repo, we use a
/// heuristic approach: capture the event signature and raw data, then
/// attempt to decode common patterns.
///
/// Known event names: Settled, SettledWithPermit
pub fn decode_proxy_event(log: &Log) -> Option<ProxySettlementEvent> {
    if log.topics.is_empty() {
        return None;
    }

    let event_sig = Hex(&log.topics[0]).to_string();
    let raw_data = Hex(&log.data).to_string();

    // Attempt to decode based on common proxy event patterns.
    // The proxy likely emits events with indexed token, payer, and recipient addresses.
    //
    // Expected pattern (3 indexed + data):
    //   topic[0] = event signature
    //   topic[1] = token address (indexed)
    //   topic[2] = payer address (indexed)
    //   topic[3] = recipient address (indexed)
    //   data     = amount (uint256) + possibly more fields
    //
    // Alternative pattern (some indexed):
    //   topic[0] = event signature
    //   topic[1] = payer (indexed)
    //   topic[2] = recipient (indexed)
    //   data     = token + amount + ...

    let (payer, recipient, token, amount) = if log.topics.len() >= 4 && log.data.len() >= 32 {
        // Pattern: 3 indexed addresses + amount in data
        let token = Some(log.topics[1][12..32].to_vec());
        let payer = Some(log.topics[2][12..32].to_vec());
        let recipient = Some(log.topics[3][12..32].to_vec());
        let amount = Some(parse_uint256(&log.data[0..32]));
        (payer, recipient, token, amount)
    } else if log.topics.len() >= 3 && log.data.len() >= 64 {
        // Pattern: 2 indexed addresses + token and amount in data
        let payer = Some(log.topics[1][12..32].to_vec());
        let recipient = Some(log.topics[2][12..32].to_vec());
        let token = Some(log.data[12..32].to_vec());
        let amount = Some(parse_uint256(&log.data[32..64]));
        (payer, recipient, token, amount)
    } else {
        (None, None, None, None)
    };

    // Classify settlement type based on event sig uniqueness
    // We identify "settled_with_permit" if the event has more data fields
    // (permit-based settlements include additional permit parameters)
    let settlement_type = if log.data.len() > 128 {
        "settled_with_permit".to_string()
    } else {
        "settled".to_string()
    };

    Some(ProxySettlementEvent {
        settlement_type,
        event_sig,
        raw_data,
        payer,
        recipient,
        token,
        amount,
    })
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
