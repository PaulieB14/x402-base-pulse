//! x402 Base Pulse - Substreams v2.0.0
//!
//! Real-time analytics for the Coinbase x402 payment protocol on Base.
//!
//! Detects x402 settlements through two mechanisms per the x402 protocol
//! docs (https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works):
//!
//! 1. **EIP-3009 (primary)**: Facilitators settle payments by calling
//!    `transferWithAuthorization` on USDC (EIP-3009 compliant). Each call
//!    emits `AuthorizationUsed(address indexed authorizer, bytes32 indexed nonce)`
//!    paired with a `Transfer(address,address,uint256)` event.
//!
//! 2. **Permit2 proxy (secondary)**: `Settled()` / `SettledWithPermit()` events
//!    from the x402ExactPermit2Proxy contract.
//!
//! Module layers:
//! - Layer 1: Event extraction (map_x402_settlements)
//! - Layer 2: State stores (payer/recipient/facilitator volume, counts, gas)
//! - Layer 3: Analytics (map_payer_stats, map_recipient_stats, map_facilitator_stats)
//! - Layer 4: SQL sink (db_out)

mod abi;
mod pb;

use abi::{
    decode_authorization_used, decode_batch_settled, decode_erc20_transfer,
    decode_facilitator_added, decode_facilitator_removed, decode_transfer_with_auth,
    format_address, is_settled_event, is_settled_with_permit_event,
};
use hex_literal::hex;
use pb::x402::v1 as x402;
use substreams::prelude::*;
use substreams::scalar::BigInt;
use substreams::store::{StoreAddBigInt, StoreAddInt64, StoreGet, StoreSet, StoreSetIfNotExistsInt64};
use substreams::Hex;
use substreams_database_change::pb::database::DatabaseChanges;
use substreams_database_change::tables::Tables;
use substreams_ethereum::pb::eth::v2 as eth;

// =============================================
// Contract addresses on Base mainnet
// Per: https://docs.cdp.coinbase.com/x402/network-support
// =============================================

/// USDC on Base mainnet - EIP-3009 compliant token
const USDC: [u8; 20] = hex!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

/// x402 Permit2 settlement proxy — the deployed CREATE2 address on Base.
/// Verified on-chain 2026-07-28: the previous 0x4020615…/0x4020633… addresses
/// carry NO contract code; 0x402085c2…0001 is the live proxy (also used by Pinax's
/// evm-x402). The old dead addresses meant Path 2 caught zero proxy settlements.
const X402_PROXY: [u8; 20] = hex!("402085c248eea27d92e8b30b2c58ed07f9e20001");

/// FacilitatorRegistry on Base - tracks authorized x402 facilitator addresses
const FACILITATOR_REGISTRY: [u8; 20] = hex!("67C75c4FD5BbbF5f6286A1874fe2d7dF0024Ebe8");

// Static facilitator allowlist — sourced from x402scan's `facilitators` npm package
// (https://github.com/Merit-Systems/x402scan/tree/main/packages/external/facilitators).
// 112 unique addresses across 29 facilitator operators.
// The on-chain FacilitatorRegistry is sparsely populated in practice (only Meridian as
// of 2026-05), so a published allowlist is required to capture real x402 activity.
// Matches gating used by x402scan + x402-omnigraph subgraph.
const STATIC_FACILITATORS: &[&str] = &[
    "0x001ddabba5782ee48842318bd9ff4008647c8d9c",  // Coinbase
    "0x0168f80e035ea68b191faf9bfc12778c87d92008",  // X402rs
    "0x021cc47adeca6673def958e324ca38023b80a5be",  // Heurist
    "0x03a3f7ce8e21e6f8d9fa14c67d8876b2470dc2f1",  // PayAI
    "0x052aaae3cad5c095850246f8ffb228354c56752a",  // Thirdweb
    "0x06f0bfd2c8f36674df5cde852c1eed8025c268c9",  // Corbits
    "0x103040545ac5031a11e8c03dd11324c7333a13c7",  // Ultravioleta DAO
    "0x1363c7ff51ccce10258a7f7bddd63baab6aaf678",  // Daydreams
    "0x15e2e2da7539ef1f652aa3c1d6142a535aa3d7ea",  // Bitrefill
    "0x16e47d275198ed65916a560bab4af6330c36ae09",  // Openmid
    "0x179761d9eed0f0d1599330cc94b0926e68ae87f1",  // AnySpend
    "0x1892f72fdb3a966b2ad8595aa5f7741ef72d6085",  // RelAI
    "0x1fc230ee3c13d0d520d49360a967dbd1555c8326",  // Heurist
    "0x222c4367a2950f3b53af260e111fc3060b0983ff",  // AurraCloud
    "0x24d4f332d8e886fc005bb4a103bad21d9ebc2b7f",  // FluxA
    "0x25659315106580ce2a787ceec5efb2d347b539c9",  // PayAI
    "0x279e08f711182c79ba6d09669127a426228a4653",  // Daydreams
    "0x290d8b8edcafb25042725cb9e78bcac36b8865f8",  // Heurist
    "0x2bb201f1bb056eb738718bd7a3ad1bef24b883bb",  // Cascade
    "0x2daaef6f941de214bf7d6daf322bc6bc7406accb",  // PayAI
    "0x2fae4026a31f19183947f0a6045ef975ebfa9ca8",  // PayAI
    "0x3210d7b21bfe1083c9dddbe17e8f947c9029a584",  // Meridian
    "0x37dfb4033d5dd98fd335f24d0d42e8fe68d587d6",  // Primer
    "0x3a5ca1c6aa6576ae9c1c0e7fa2b4883346bc5aa0",  // Thirdweb
    "0x3a70788150c7645a21b95b7062ab1784d3cc2104",  // Coinbase
    "0x3be45f576696a2fd5a93c1330cd19f1607ab311d",  // xEcho
    "0x3f61093f61817b29d9556d3b092e67746af8cdfd",  // Heurist
    "0x40272e2eac848ea70db07fd657d799bd309329c4",  // Dexter
    "0x402feee072d655b85e08f1751af9ddbcd249521f",  // Dexter
    "0x4544b535938b67d2a410a98a7e3b0f8f68921ca7",  // Questflow
    "0x4638bc811c93bf5e60deed32325e93505f681576",  // Questflow
    "0x47d8b3c9717e976f31025089384f23900750a5f4",  // Coinbase
    "0x489c40fc3c2a19ad8cb275b7dd6aa194e9219c4f",  // PayAI
    "0x48ab4b0af4ddc2f666a3fcc43666c793889787a3",  // Heurist
    "0x4ffeffa616a1460570d1eb0390e264d45a199e91",  // Coinbase
    "0x51fec16843e49b99aaf9814e525aee1756e66a62",  // x402 Jobs
    "0x552300992857834c0ad41c8e1a6934a5e4a2e4ca",  // Coinbase
    "0x59e8014a3b884392fbb679fe461da07b18c1ff81",  // Questflow
    "0x5e437bee4321db862ac57085ea5eb97199c0ccc5",  // X402rs
    "0x612d72dc8402bba997c61aa82ce718ea23b2df5d",  // Heurist
    "0x65058cf664d0d07f68b663b0d4b4f12a5e331a38",  // CodeNut
    "0x66c40946b0dffd04be467e18309857307ecd37cb",  // Polymer
    "0x675707bc7d03089f820c1b7d49f7480083e8f4df",  // PayAI
    "0x67b9ce703d9ce658d7c4ac3c289cea112fe662af",  // Coinbase
    "0x6831508455a716f987782a1ab41e204856055cc2",  // Coinbase
    "0x68a96f41ff1e9f2e7b591a931a4ad224e7c07863",  // Coinbase
    "0x6ccf245c883f9f3c6caee0687aa61daf7bc96e32",  // PayAI
    "0x708e57b6650a9a741ab39cae1969ea1d2d10eca1",  // Coinbase
    "0x724efafb051f17ae824afcdf3c0368ae312da264",  // Questflow
    "0x73b2b8df52fbe7c40fe78db52e3dffdd5db5ad07",  // 402104
    "0x76eee8f0acabd6b49f1cc4e9656a0c8892f3332e",  // X402rs
    "0x7c766f5fd9ab3dc09acad5ecfacc99c4781efe29",  // OpenFacilitator
    "0x7e20b62bf36554b704774afb0fcc0ae8f899213b",  // Thirdweb
    "0x7f6d822467df2a85f792d4508c5722ade96be056",  // Coinbase
    "0x7f72a02c682e908d46a5677fe937cdb612d94a3b",  // FluxA
    "0x80735b3f7808e2e229ace880dbe85e80115631ca",  // Virtuals Protocol
    "0x80c08de1a05df2bd633cf520754e40fde3c794d3",  // Thirdweb
    "0x87af99356d774312b73018b3b6562e1ae0e018c9",  // CodeNut
    "0x88800e08e20b45c9b1f0480cf759b5bf2f05180c",  // Coinbase
    "0x88e13d4c764a6c840ce722a0a3765f55a85b327e",  // CodeNut
    "0x8d8fa42584a727488eeb0e29405ad794a105bb9b",  // CodeNut
    "0x8e7769d440b3460b92159dd9c6d17302b036e2d6",  // Meridian
    "0x8f5cb67b49555e614892b7233cfddebfb746e531",  // Coinbase
    "0x90d5e567017f6c696f1916f4365dd79985fce50f",  // Heurist
    "0x90da501fdbec74bb0549100967eb221fed79c99b",  // Questflow
    "0x91d313853ad458addda56b35a7686e2f38ff3952",  // Coinbase
    "0x91ddea05f741b34b63a7548338c90fc152c8631f",  // Thirdweb
    "0x94701e1df9ae06642bf6027589b8e05dc7004813",  // Coinbase
    "0x97316fa4730bc7d3b295234f8e4d04a0a4c093e8",  // OpenX402
    "0x97acce27d5069544480bde0f04d9f47d7422a016",  // Coinbase
    "0x97d38aa5de015245dcca76305b53abe6da25f6a5",  // X402rs
    "0x97db9b5291a218fc77198c285cefdc943ef74917",  // OpenX402
    "0x9aae2b0d1b9dc55ac9bab9556f9a26cb64995fb9",  // Coinbase
    "0x9c09faa49c4235a09677159ff14f17498ac48738",  // Coinbase
    "0x9df61a719ddae27c20a63a417271cc2c704654bd",  // PayAI
    "0x9fb2714af0a84816f5c6322884f2907e33946b88",  // Coinbase
    "0xa1822b21202a24669eaf9277723d180cd6dae874",  // Thirdweb
    "0xa32ccda98ba7529705a059bd2d213da8de10d101",  // Coinbase
    "0xa9a54ef09fc8b86bc747cec6ef8d6e81c38c6180",  // Questflow
    "0xaa0df01e4d11decf2ad2c459c81d3a495e4f1925",  // FluxA
    "0xaaca1ba9d2627cbc0739ba69890c30f95de046e4",  // Thirdweb
    "0xadd5585c776b9b0ea77e9309c1299a40442d820f",  // Coinbase
    "0xaf990eef9846b63d896056050fdc0b28bca9c24b",  // PayAI
    "0xb2bd29925cbbcea7628279c91945ca5b98bf371b",  // PayAI
    "0xb578b7db22581507d62bdbeb85e06acd1be09e11",  // Heurist
    "0xb5d25e1fa0718bf3e1bf698f96791d4e93632ec8",  // FluxA
    "0xb70c4fe126de09bd292fe3d1e40c6d264ca6a52a",  // AurraCloud
    "0xb8f41cb13b1f213da1e94e1b742ec1323235c48f",  // PayAI
    "0xc19829b32324f116ee7f80d193f99e445968499a",  // X402rs
    "0xc6699d2aada6c36dfea5c248dd70f9cb0235cb63",  // PayAI
    "0xc67b555b4a9d340ed7c5d87743163c31a75f2254",  // FluxA
    "0xcbb10c30a9a72fae9232f41cbbd566a097b4e03a",  // Coinbase
    "0xce7819f0b0b871733c933d1f486533bab95ec47b",  // Questflow
    "0xce82eeec8e98e443ec34fda3c3e999cbe4cb6ac2",  // Coinbase
    "0xd2f74a14522d40e4a1d7fbb62aa97ce99fa1a7e5",  // FluxA
    "0xd348e724e0ef36291a28dfeccf692399b0e179f8",  // AurraCloud
    "0xd7469bf02d221968ab9f0c8b9351f55f8668ac4f",  // Coinbase
    "0xd7d91a42dfadd906c5b9ccde7226d28251e4cd0f",  // Questflow
    "0xd88a9a58806b895ff06744082c6a20b9d7184b0f",  // Thirdweb
    "0xd8dfc729cbd05381647eb5540d756f4f8ad63eec",  // X402rs
    "0xd97c12726dcf994797c981d31cfb243d231189fb",  // Heurist
    "0xdbdf3d8ed80f84c35d01c6c9f9271761bad90ba6",  // Coinbase
    "0xdc8fbad54bf5151405de488f45acd555517e0958",  // Coinbase
    "0xe07e9cbf9a55d02e3ac356ed4706353d98c5a618",  // Treasure
    "0xe299c486066739c4a31609e1268d93229632dd47",  // PayAI
    "0xe575fa51af90957d66fab6d63355f1ed021b887b",  // PayAI
    "0xe6123e6b389751c5f7e9349f3d626b105c1fe618",  // Questflow
    "0xea52f2c6f6287f554f9b54c5417e1e431fe5710e",  // Thirdweb
    "0xec10243b54df1a71254f58873b389b7ecece89c2",  // Thirdweb
    "0xf46833d4ac4f0f1405cc05c30edfd86770f721c9",  // PayAI
    "0xf70e7cb30b132fab2a0a5e80d41861aa133ea21b",  // Questflow
    "0xfe0920a0a7f0f8a1ec689146c30c3bbef439bf8a",  // Mogami
];

fn is_static_facilitator(addr_lower: &str) -> bool {
    STATIC_FACILITATORS.binary_search(&addr_lower).is_ok()
}

// Null / zero address
const ZERO_ADDR: &str = "0x0000000000000000000000000000000000000000";

substreams_ethereum::init!();

/// Convert Unix timestamp seconds to PostgreSQL TIMESTAMP format
fn unix_to_timestamp(secs: i64) -> String {
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let mut days = days_since_epoch;
    let mut year = 1970i64;
    loop {
        let diy = if is_leap_year(year) { 366 } else { 365 };
        if days < diy {
            break;
        }
        days -= diy;
        year += 1;
    }

    let dim: [i64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &d in &dim {
        if days < d {
            break;
        }
        days -= d;
        month += 1;
    }
    let day = days + 1;

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

/// Extract gas_price from a protobuf BigInt (big-endian signed bytes) as a string
fn proto_bigint_to_string(bi: &eth::BigInt) -> String {
    if bi.bytes.is_empty() {
        return "0".to_string();
    }
    let val = num_bigint::BigInt::from_signed_bytes_be(&bi.bytes);
    val.to_string()
}

// =============================================
// LAYER 0: Facilitator Registry
// =============================================

/// Extract FacilitatorAdded and FacilitatorRemoved events from the on-chain
/// FacilitatorRegistry contract.
#[substreams::handlers::map]
fn map_facilitator_registry_events(
    blk: eth::Block,
) -> Result<x402::FacilitatorRegistryEvents, substreams::errors::Error> {
    let mut events = x402::FacilitatorRegistryEvents {
        block_number: blk.number,
        ..Default::default()
    };

    for log in blk.logs() {
        let log_entry = log.log;
        if log_entry.address != FACILITATOR_REGISTRY {
            continue;
        }

        if let Some(added) = decode_facilitator_added(log_entry) {
            events.events.push(x402::FacilitatorRegistryEvent {
                facilitator_address: format_address(&added.facilitator),
                name: added.name,
                url: added.url,
                is_added: true,
            });
        } else if let Some(removed) = decode_facilitator_removed(log_entry) {
            events.events.push(x402::FacilitatorRegistryEvent {
                facilitator_address: format_address(&removed.facilitator),
                name: String::new(),
                url: String::new(),
                is_added: false,
            });
        }
    }

    Ok(events)
}

/// Maintain the set of registered facilitators. Key: address, Value: "name|url" or "" if removed.
#[substreams::handlers::store]
fn store_facilitator_registry(
    events: x402::FacilitatorRegistryEvents,
    store: StoreSetString,
) {
    for event in events.events {
        let key = event.facilitator_address.to_lowercase();
        if event.is_added {
            let val = format!("{}|{}", event.name, event.url);
            store.set(0, &key, &val);
        } else {
            // Mark as removed with empty string
            let empty = String::new();
            store.set(0, &key, &empty);
        }
    }
}

// =============================================
// LAYER 1: Event Extraction
// =============================================

/// Extract x402 settlements by detecting EIP-3009 AuthorizationUsed events
/// on the USDC contract. EIP-3009 settlements are gated by the FacilitatorRegistry.
///
/// Per the x402 protocol (https://docs.cdp.coinbase.com/x402/core-concepts/how-it-works),
/// facilitators settle payments by calling `transferWithAuthorization` on USDC.
/// Each `AuthorizationUsed(address indexed authorizer, bytes32 indexed nonce)`
/// event is paired with its corresponding `Transfer(address,address,uint256)`
/// event to capture payer, recipient, and amount.
///
/// Also detects Permit2 proxy settlements (Settled / SettledWithPermit) from
/// the x402ExactPermit2Proxy contract for the newer settlement path.
#[substreams::handlers::map]
fn map_x402_settlements(
    blk: eth::Block,
    registry_store: StoreGetString,
) -> Result<x402::Settlements, substreams::errors::Error> {
    let mut settlements = x402::Settlements {
        block_number: blk.number,
        block_timestamp: Some(blk.timestamp().clone()),
        ..Default::default()
    };

    for trx in blk.transaction_traces.iter() {
        let receipt = match trx.receipt.as_ref() {
            Some(r) => r,
            None => continue,
        };

        // -----------------------------------------------
        // Path 1: EIP-3009 AuthorizationUsed on USDC
        // Facilitator calls transferWithAuthorization on USDC.
        // USDC emits AuthorizationUsed + Transfer events.
        // -----------------------------------------------
        let auth_events: Vec<_> = receipt
            .logs
            .iter()
            .filter(|log| log.address == USDC)
            .filter_map(|log| decode_authorization_used(log))
            .collect();

        if !auth_events.is_empty() {
            // Gate: process if tx.from is either (a) on-chain registered via
            // FacilitatorRegistry OR (b) on the published static allowlist
            // (x402scan's facilitators package). The on-chain registry is
            // sparsely populated in practice, so without the allowlist
            // virtually no real x402 activity would be indexed.
            let facilitator_addr = format_address(&trx.from).to_lowercase();
            let in_registry = registry_store.get_last(&facilitator_addr).is_some();
            let in_allowlist = is_static_facilitator(&facilitator_addr);
            if !in_registry && !in_allowlist {
                continue;
            }

            // Collect Transfer events from USDC in this transaction
            let transfer_events: Vec<_> = receipt
                .logs
                .iter()
                .filter(|log| log.address == USDC)
                .filter_map(|log| decode_erc20_transfer(log))
                .collect();

            let facilitator = format_address(&trx.from);
            let gas_used = trx.gas_used.to_string();
            let gas_price = trx
                .gas_price
                .as_ref()
                .map(|p| proto_bigint_to_string(p))
                .unwrap_or_else(|| "0".to_string());

            // Check if this tx also has proxy events (hybrid detection)
            let has_proxy_settled = receipt.logs.iter().any(|log| {
                log.address == X402_PROXY
                    && (is_settled_event(log) || is_settled_with_permit_event(log))
            });

            // Recover the EIP-3009 validity window (validAfter/validBefore) by
            // decoding the transferWithAuthorization call(s) in this tx, keyed by
            // nonce so each AuthorizationUsed event can look up its own window.
            let mut validity: std::collections::HashMap<Vec<u8>, (u64, u64)> =
                std::collections::HashMap::new();
            for call in trx.calls.iter() {
                if call.address == USDC {
                    if let Some(twa) = decode_transfer_with_auth(&call.input) {
                        validity.insert(twa.nonce, (twa.valid_after, twa.valid_before));
                    }
                }
            }

            for auth in &auth_events {
                // Find the corresponding Transfer event for this authorization.
                // In USDC's implementation, transferWithAuthorization emits
                // AuthorizationUsed then Transfer, so we look for a Transfer
                // where from == authorizer with log_index > auth.log_index.
                let transfer = transfer_events
                    .iter()
                    .filter(|t| t.from == auth.authorizer && t.log_index > auth.log_index)
                    .min_by_key(|t| t.log_index);

                let (payer, recipient, amount) = if let Some(t) = transfer {
                    (
                        format_address(&auth.authorizer),
                        format_address(&t.to),
                        t.amount.clone(),
                    )
                } else {
                    // AuthorizationUsed without a matching Transfer (shouldn't happen
                    // in normal USDC operation, but handle gracefully)
                    (format_address(&auth.authorizer), String::new(), "0".to_string())
                };

                let settlement_type = if has_proxy_settled {
                    "eip3009_proxy".to_string()
                } else {
                    "eip3009".to_string()
                };

                let nonce = Hex(&auth.nonce).to_string();
                let (valid_after, valid_before) =
                    validity.get(&auth.nonce).copied().unwrap_or((0, 0));

                settlements.settlements.push(x402::Settlement {
                    id: format!("{}-{}", Hex(&trx.hash).to_string(), auth.log_index),
                    tx_hash: Hex(&trx.hash).to_string(),
                    log_index: auth.log_index,
                    block_number: blk.number,
                    timestamp: Some(blk.timestamp().clone()),
                    payer,
                    recipient,
                    token: format_address(&USDC),
                    amount,
                    settlement_type,
                    scheme: "exact".to_string(),
                    facilitator: facilitator.clone(),
                    gas_used: gas_used.clone(),
                    gas_price: gas_price.clone(),
                    nonce,
                    valid_after,
                    valid_before,
                });
            }

            continue; // EIP-3009 path handled this tx
        }

        // -----------------------------------------------
        // Path 2: Permit2 proxy (Settled / SettledWithPermit)
        // x402ExactPermit2Proxy emits parameterless Settled() or
        // SettledWithPermit() events. We correlate each with its
        // corresponding USDC Transfer event in the same tx.
        // -----------------------------------------------
        let proxy_events: Vec<_> = receipt
            .logs
            .iter()
            .filter(|log| {
                log.address == X402_PROXY
                    && (is_settled_event(log) || is_settled_with_permit_event(log))
            })
            .collect();

        // Path 3: batch-settlement channel sweeps — x402BatchSettlement emits
        // Settled(receiver, token, sender, amount) on redemption. Gate by
        // token == USDC. (Forward-looking: 0 Base volume as of 2026-07-28.)
        let batch_events: Vec<_> = receipt
            .logs
            .iter()
            .filter_map(|log| decode_batch_settled(log))
            .filter(|b| b.token == USDC)
            .collect();

        if proxy_events.is_empty() && batch_events.is_empty() {
            continue;
        }

        let facilitator = format_address(&trx.from);
        let gas_used = trx.gas_used.to_string();
        let gas_price = trx
            .gas_price
            .as_ref()
            .map(|p| proto_bigint_to_string(p))
            .unwrap_or_else(|| "0".to_string());

        // Collect USDC transfers for proxy correlation
        let usdc_transfers: Vec<_> = receipt
            .logs
            .iter()
            .filter(|log| log.address == USDC)
            .filter_map(|log| decode_erc20_transfer(log))
            .collect();

        for (i, proxy_log) in proxy_events.iter().enumerate() {
            let settlement_type = if is_settled_with_permit_event(proxy_log) {
                "settled_with_permit".to_string()
            } else {
                "settled".to_string()
            };

            // Pair each proxy event with its corresponding USDC transfer by position
            let (payer, recipient, amount) = usdc_transfers
                .get(i)
                .map(|t| (format_address(&t.from), format_address(&t.to), t.amount.clone()))
                .unwrap_or_else(|| (facilitator.clone(), String::new(), "0".to_string()));

            settlements.settlements.push(x402::Settlement {
                id: format!("{}-{}", Hex(&trx.hash).to_string(), proxy_log.index),
                tx_hash: Hex(&trx.hash).to_string(),
                log_index: proxy_log.index,
                block_number: blk.number,
                timestamp: Some(blk.timestamp().clone()),
                payer,
                recipient,
                token: format_address(&USDC),
                amount,
                settlement_type,
                scheme: "exact".to_string(),
                facilitator: facilitator.clone(),
                gas_used: gas_used.clone(),
                gas_price: gas_price.clone(),
                nonce: String::new(),
                valid_after: 0,
                valid_before: 0,
            });
        }

        // Path 3: emit batch-settlement channel sweeps. recipient + amount + token
        // are exact; payer is the on-chain settler (approx) pending channel-level
        // attribution (v3.3, once batch has live volume).
        for b in &batch_events {
            settlements.settlements.push(x402::Settlement {
                id: format!("{}-{}", Hex(&trx.hash).to_string(), b.log_index),
                tx_hash: Hex(&trx.hash).to_string(),
                log_index: b.log_index,
                block_number: blk.number,
                timestamp: Some(blk.timestamp().clone()),
                payer: format_address(&b.sender),
                recipient: format_address(&b.receiver),
                token: format_address(&b.token),
                amount: b.amount.clone(),
                settlement_type: "batch_settlement".to_string(),
                scheme: "batch".to_string(),
                facilitator: facilitator.clone(),
                gas_used: gas_used.clone(),
                gas_price: gas_price.clone(),
                nonce: String::new(),
                valid_after: 0,
                valid_before: 0,
            });
        }
    }

    Ok(settlements)
}

// =============================================
// LAYER 2: State Stores
// =============================================

/// Accumulate total payment volume per payer
#[substreams::handlers::store]
fn store_payer_volume(settlements: x402::Settlements, store: StoreAddBigInt) {
    for s in settlements.settlements {
        if s.payer.is_empty() || s.payer == ZERO_ADDR {
            continue;
        }
        let amount = BigInt::try_from(&s.amount).unwrap_or_else(|_| BigInt::zero());
        store.add(0, &s.payer.to_lowercase(), &amount);
    }
}

/// Count total payments per payer
#[substreams::handlers::store]
fn store_payer_count(settlements: x402::Settlements, store: StoreAddInt64) {
    for s in settlements.settlements {
        if s.payer.is_empty() || s.payer == ZERO_ADDR {
            continue;
        }
        store.add(0, &s.payer.to_lowercase(), 1);
    }
}

/// Accumulate total revenue per recipient (resource server)
#[substreams::handlers::store]
fn store_recipient_volume(settlements: x402::Settlements, store: StoreAddBigInt) {
    for s in settlements.settlements {
        if s.recipient.is_empty() || s.recipient == ZERO_ADDR {
            continue;
        }
        let amount = BigInt::try_from(&s.amount).unwrap_or_else(|_| BigInt::zero());
        store.add(0, &s.recipient.to_lowercase(), &amount);
    }
}

/// Count total payments per recipient
#[substreams::handlers::store]
fn store_recipient_count(settlements: x402::Settlements, store: StoreAddInt64) {
    for s in settlements.settlements {
        if s.recipient.is_empty() || s.recipient == ZERO_ADDR {
            continue;
        }
        store.add(0, &s.recipient.to_lowercase(), 1);
    }
}

/// Accumulate total volume settled per facilitator
#[substreams::handlers::store]
fn store_facilitator_volume(settlements: x402::Settlements, store: StoreAddBigInt) {
    for s in settlements.settlements {
        if s.facilitator.is_empty() {
            continue;
        }
        let amount = BigInt::try_from(&s.amount).unwrap_or_else(|_| BigInt::zero());
        store.add(0, &s.facilitator.to_lowercase(), &amount);
    }
}

/// Count total settlements per facilitator
#[substreams::handlers::store]
fn store_facilitator_count(settlements: x402::Settlements, store: StoreAddInt64) {
    for s in settlements.settlements {
        if s.facilitator.is_empty() {
            continue;
        }
        store.add(0, &s.facilitator.to_lowercase(), 1);
    }
}

/// Accumulate total gas cost per facilitator (gas_used * gas_price in wei)
#[substreams::handlers::store]
fn store_facilitator_gas(settlements: x402::Settlements, store: StoreAddBigInt) {
    for s in settlements.settlements {
        if s.facilitator.is_empty() {
            continue;
        }
        let gas_used = BigInt::try_from(&s.gas_used).unwrap_or_else(|_| BigInt::zero());
        let gas_price = BigInt::try_from(&s.gas_price).unwrap_or_else(|_| BigInt::zero());
        let gas_cost = gas_used * gas_price;
        store.add(0, &s.facilitator.to_lowercase(), &gas_cost);
    }
}

/// Record the first-seen block timestamp per payer, recipient, and facilitator.
/// Uses set_if_not_exists so only the earliest timestamp is stored.
#[substreams::handlers::store]
fn store_first_seen(settlements: x402::Settlements, store: StoreSetIfNotExistsInt64) {
    let ts = settlements
        .block_timestamp
        .as_ref()
        .map(|t| t.seconds)
        .unwrap_or(0);
    for s in settlements.settlements {
        if !s.payer.is_empty() && s.payer != ZERO_ADDR {
            store.set_if_not_exists(0, format!("payer:{}", s.payer.to_lowercase()), &ts);
        }
        if !s.recipient.is_empty() && s.recipient != ZERO_ADDR {
            store.set_if_not_exists(0, format!("recipient:{}", s.recipient.to_lowercase()), &ts);
        }
        if !s.facilitator.is_empty() {
            store.set_if_not_exists(
                0,
                format!("facilitator:{}", s.facilitator.to_lowercase()),
                &ts,
            );
        }
    }
}

// =============================================
// LAYER 3: Analytics
// =============================================

/// Compute aggregated payer statistics
#[substreams::handlers::map]
fn map_payer_stats(
    settlements: x402::Settlements,
    volume_deltas: Deltas<DeltaBigInt>,
    count_store: StoreGetInt64,
    first_seen_store: StoreGetInt64,
) -> Result<x402::PayerStats, substreams::errors::Error> {
    let mut stats = x402::PayerStats {
        block_number: settlements.block_number,
        ..Default::default()
    };

    for delta in volume_deltas.deltas {
        let payer = delta.key.clone();
        let total_payments = count_store.get_last(&payer).unwrap_or(0) as u64;
        let first_payment_at = first_seen_store
            .get_last(&format!("payer:{}", payer))
            .map(|secs| prost_types::Timestamp { seconds: secs, nanos: 0 });

        stats.stats.push(x402::PayerStat {
            payer_address: payer,
            total_spent: delta.new_value.to_string(),
            total_payments,
            first_payment_at,
            last_payment_at: settlements.block_timestamp.clone(),
        });
    }

    Ok(stats)
}

/// Compute aggregated recipient (resource server) statistics
#[substreams::handlers::map]
fn map_recipient_stats(
    settlements: x402::Settlements,
    volume_deltas: Deltas<DeltaBigInt>,
    count_store: StoreGetInt64,
    first_seen_store: StoreGetInt64,
) -> Result<x402::RecipientStats, substreams::errors::Error> {
    let mut stats = x402::RecipientStats {
        block_number: settlements.block_number,
        ..Default::default()
    };

    for delta in volume_deltas.deltas {
        let recipient = delta.key.clone();
        let total_payments = count_store.get_last(&recipient).unwrap_or(0) as u64;
        let first_payment_at = first_seen_store
            .get_last(&format!("recipient:{}", recipient))
            .map(|secs| prost_types::Timestamp { seconds: secs, nanos: 0 });

        stats.stats.push(x402::RecipientStat {
            recipient_address: recipient,
            total_received: delta.new_value.to_string(),
            total_payments,
            first_payment_at,
            last_payment_at: settlements.block_timestamp.clone(),
        });
    }

    Ok(stats)
}

/// Compute facilitator economics, enriched with name and active status from
/// the FacilitatorRegistry.
#[substreams::handlers::map]
fn map_facilitator_stats(
    settlements: x402::Settlements,
    volume_deltas: Deltas<DeltaBigInt>,
    count_store: StoreGetInt64,
    gas_store: StoreGetBigInt,
    first_seen_store: StoreGetInt64,
    registry_store: StoreGetString,
) -> Result<x402::FacilitatorStats, substreams::errors::Error> {
    let mut stats = x402::FacilitatorStats {
        block_number: settlements.block_number,
        ..Default::default()
    };

    for delta in volume_deltas.deltas {
        let facilitator = delta.key.clone();
        let total_settlements = count_store.get_last(&facilitator).unwrap_or(0) as u64;
        let total_gas = gas_store
            .get_last(&facilitator)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "0".to_string());
        let first_settlement_at = first_seen_store
            .get_last(&format!("facilitator:{}", facilitator))
            .map(|secs| prost_types::Timestamp { seconds: secs, nanos: 0 });

        // Look up facilitator name and status from registry
        let (name, url, is_active) = match registry_store.get_last(&facilitator) {
            Some(val) if !val.is_empty() => {
                let parts: Vec<&str> = val.splitn(2, '|').collect();
                let name = parts.first().unwrap_or(&"").to_string();
                let url = parts.get(1).unwrap_or(&"").to_string();
                (name, url, true)
            }
            Some(_) => (String::new(), String::new(), false), // Removed facilitator
            None => (String::new(), String::new(), false),     // Unknown facilitator
        };

        stats.stats.push(x402::FacilitatorStat {
            facilitator_address: facilitator,
            total_settlements,
            total_volume_settled: delta.new_value.to_string(),
            total_gas_spent: total_gas,
            first_settlement_at,
            last_settlement_at: settlements.block_timestamp.clone(),
            name,
            is_active,
            url,
        });
    }

    Ok(stats)
}

// =============================================
// LAYER 4: SQL Sink
// =============================================

/// Output database changes for PostgreSQL
#[substreams::handlers::map]
fn db_out(
    params: String,
    settlements: x402::Settlements,
    payer_stats: x402::PayerStats,
    recipient_stats: x402::RecipientStats,
    facilitator_stats: x402::FacilitatorStats,
) -> Result<DatabaseChanges, substreams::errors::Error> {
    let mut tables = Tables::new();

    // Parse min_amount param
    let min_amount = params
        .split('=')
        .nth(1)
        .map(|v| v.to_string())
        .and_then(|v| BigInt::try_from(&v).ok())
        .unwrap_or_else(BigInt::zero);

    // Insert settlements
    for s in settlements.settlements {
        let amount = BigInt::try_from(&s.amount).unwrap_or_else(|_| BigInt::zero());
        if amount < min_amount {
            continue;
        }

        let timestamp = s
            .timestamp
            .as_ref()
            .map(|t| unix_to_timestamp(t.seconds))
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());

        tables
            .create_row("settlements", &s.id)
            .set("block_number", s.block_number)
            .set("block_timestamp", &timestamp)
            .set("tx_hash", &s.tx_hash)
            .set("log_index", s.log_index)
            .set("payer", &s.payer)
            .set("recipient", &s.recipient)
            .set("token", &s.token)
            .set("amount", &s.amount)
            .set("settlement_type", &s.settlement_type)
            .set("scheme", &s.scheme)
            .set("facilitator", &s.facilitator)
            .set("gas_used", &s.gas_used)
            .set("gas_price", &s.gas_price)
            .set("nonce", &s.nonce)
            .set("valid_after", s.valid_after as i64)
            .set("valid_before", s.valid_before as i64);
    }

    // Upsert payer stats
    for stat in payer_stats.stats {
        let first_ts = stat.first_payment_at.as_ref()
            .map(|t| unix_to_timestamp(t.seconds))
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
        let last_ts = stat.last_payment_at.as_ref()
            .map(|t| unix_to_timestamp(t.seconds))
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
        tables
            .create_row("payers", &stat.payer_address)
            .set("total_spent", stat.total_spent.as_str())
            .set("total_payments", stat.total_payments as i64)
            .set("first_payment_at", &first_ts)
            .set("last_payment_at", &last_ts);
    }

    // Upsert recipient stats
    for stat in recipient_stats.stats {
        let first_ts = stat.first_payment_at.as_ref()
            .map(|t| unix_to_timestamp(t.seconds))
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
        let last_ts = stat.last_payment_at.as_ref()
            .map(|t| unix_to_timestamp(t.seconds))
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
        tables
            .create_row("recipients", &stat.recipient_address)
            .set("total_received", stat.total_received.as_str())
            .set("total_payments", stat.total_payments as i64)
            .set("first_payment_at", &first_ts)
            .set("last_payment_at", &last_ts);
    }

    // Upsert facilitator stats
    for stat in facilitator_stats.stats {
        let first_ts = stat.first_settlement_at.as_ref()
            .map(|t| unix_to_timestamp(t.seconds))
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
        let last_ts = stat.last_settlement_at.as_ref()
            .map(|t| unix_to_timestamp(t.seconds))
            .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
        tables
            .create_row("facilitators", &stat.facilitator_address)
            .set("name", &stat.name)
            .set("url", &stat.url)
            .set("is_active", stat.is_active)
            .set("total_settlements", stat.total_settlements as i64)
            .set("total_volume_settled", stat.total_volume_settled.as_str())
            .set("total_gas_spent", stat.total_gas_spent.as_str())
            .set("first_settlement_at", &first_ts)
            .set("last_settlement_at", &last_ts);
    }

    Ok(tables.to_database_changes())
}
