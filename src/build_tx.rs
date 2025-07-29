use bitcoin::{
    absolute,
    address::Address,
    opcodes::all::OP_PUSHNUM_1,
    psbt::Psbt,
    script::{Builder, PushBytesBuf},
    transaction::{OutPoint, Transaction, TxIn, TxOut},
    Amount, Network, ScriptBuf, Sequence, Txid, Witness,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::str::FromStr;

// Module-level constant for the rune we're working with
const RUNE_NAME: &str = "TESTSLUGLINERUNE";

#[derive(Debug, Deserialize, Serialize)]
struct RuneInfo {
    amount: u64,
    divisibility: u8,
    symbol: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Utxo {
    address: String,
    confirmations: u32,
    indexed: bool,
    inscriptions: Vec<String>,
    outpoint: String,
    runes: HashMap<String, RuneInfo>,
    sat_ranges: Option<Vec<String>>,
    script_pubkey: String,
    spent: bool,
    transaction: String,
    value: u64,
}

fn parse_network(network_str: &str) -> Network {
    match network_str {
        "testnet" | "testnet4" => Network::Testnet,
        "signet" => Network::Signet,
        "regtest" => Network::Regtest,
        _ => Network::Bitcoin,
    }
}

fn build_transaction(
    selected_utxos: &[&Utxo],
    rune_utxos: &[Utxo],
    btc_address: &str,
    destination_address: &str,
    amount: u64,
    network: Network,
) -> Result<Transaction, Box<dyn Error>> {
    // Parse addresses
    let dest_addr = Address::from_str(destination_address)?
        .require_network(network)?;
    let change_addr = Address::from_str(btc_address)?
        .require_network(network)?;
    
    // Calculate total input value from BTC UTXOs
    let btc_input: u64 = selected_utxos.iter().map(|u| u.value).sum();
    
    // Check if we have at least one rune UTXO
    if rune_utxos.is_empty() {
        return Err("No rune UTXOs available for fee payment".into());
    }
    
    // Create inputs from selected UTXOs
    let mut inputs = Vec::new();
    for utxo in selected_utxos {
        // Parse outpoint in format "txid:vout"
        let parts: Vec<&str> = utxo.outpoint.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid outpoint format: {}", utxo.outpoint).into());
        }
        let txid = Txid::from_str(parts[0])?;
        let vout: u32 = parts[1].parse()?;
        
        let outpoint = OutPoint {
            txid,
            vout,
        };
        
        inputs.push(TxIn {
            previous_output: outpoint,
            script_sig: ScriptBuf::new(), // Empty for now, will be signed later
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        });
    }
    
    // Add rune UTXO as input (just one for now)
    let rune_utxo = &rune_utxos[0];
    let parts: Vec<&str> = rune_utxo.outpoint.split(':').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid rune outpoint format: {}", rune_utxo.outpoint).into());
    }
    let txid = Txid::from_str(parts[0])?;
    let vout: u32 = parts[1].parse()?;
    
    inputs.push(TxIn {
        previous_output: OutPoint { txid, vout },
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::default(),
    });
    
    // Calculate total input including rune UTXO value
    let total_input = btc_input + rune_utxo.value;
    
    // Create outputs
    let mut outputs = Vec::new();
    
    // First output: P2A (Pay-to-Anchor) with 0 sats
    // Create P2A script: OP_1 <0x4e73>
    let push_bytes = PushBytesBuf::try_from(&[0x4e, 0x73]).unwrap();
    let p2a_script = Builder::new()
        .push_opcode(OP_PUSHNUM_1)
        .push_slice(push_bytes)
        .into_script();
    
    outputs.push(TxOut {
        value: Amount::from_sat(0),
        script_pubkey: p2a_script,
    });
    
    // Second output: destination output
    outputs.push(TxOut {
        value: Amount::from_sat(amount),
        script_pubkey: dest_addr.script_pubkey(),
    });
    
    // Add change output if there's any change
    // Note: In a real implementation, we would subtract fees here
    let change = total_input.saturating_sub(amount);
    if change > 0 {
        outputs.push(TxOut {
            value: Amount::from_sat(change),
            script_pubkey: change_addr.script_pubkey(),
        });
    }
    
    // Build the transaction (version 3)
    let tx = Transaction {
        version: bitcoin::transaction::Version(3),
        lock_time: absolute::LockTime::ZERO,
        input: inputs,
        output: outputs,
    };
    
    Ok(tx)
}

fn fetch_utxos(ord_server: &str, address: &str) -> Result<Vec<Utxo>, Box<dyn Error>> {
    let url = format!("{}/outputs/{}", ord_server, address);
    println!("Fetching UTXOs from: {}", url);
    
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .send()?;
    
    if !response.status().is_success() {
        return Err(format!("Failed to fetch UTXOs: {}", response.status()).into());
    }
    
    let utxos: Vec<Utxo> = response.json()?;
    
    // Filter out spent UTXOs
    let unspent_utxos: Vec<Utxo> = utxos.into_iter()
        .filter(|u| !u.spent)
        .collect();
    
    Ok(unspent_utxos)
}

fn fetch_rune_utxos(ord_server: &str, address: &str) -> Result<Vec<Utxo>, Box<dyn Error>> {
    let utxos = fetch_utxos(ord_server, address)?;
    
    // Filter to only UTXOs containing our target rune
    let rune_utxos: Vec<Utxo> = utxos.into_iter()
        .filter(|u| u.runes.contains_key(RUNE_NAME))
        .collect();
    
    Ok(rune_utxos)
}

fn select_utxos(utxos: &[Utxo], target_amount: u64) -> Result<Vec<&Utxo>, String> {
    // Sort UTXOs by value in descending order
    let mut sorted_utxos: Vec<&Utxo> = utxos.iter().collect();
    sorted_utxos.sort_by(|a, b| b.value.cmp(&a.value));
    
    let mut selected = Vec::new();
    let mut accumulated = 0u64;
    
    for utxo in sorted_utxos {
        selected.push(utxo);
        accumulated += utxo.value;
        
        if accumulated >= target_amount {
            return Ok(selected);
        }
    }
    
    Err(format!(
        "Insufficient funds. Available: {} sats, Required: {} sats",
        accumulated, target_amount
    ))
}

pub fn run(
    _bitcoind_host: &str,
    _bitcoind_user: Option<&str>,
    _bitcoind_password: Option<&str>,
    network: &str,
    ord_server: &str,
    btc_address: &str,
    runes_address: &str,
    destination_address: &str,
    amount: u64,
) {
    println!("Building transaction...");
    println!("BTC address: {}", btc_address);
    println!("Runes address: {}", runes_address);
    println!("Destination address: {}", destination_address);
    println!("Amount: {} sats", amount);
    println!("Network: {}", network);
    
    // Fetch BTC UTXOs
    match fetch_utxos(ord_server, btc_address) {
        Ok(utxos) => {
            println!("Found {} UTXOs", utxos.len());
            
            // Calculate total balance
            let total_balance: u64 = utxos.iter().map(|u| u.value).sum();
            println!("Total balance: {} sats", total_balance);
            
            // Select UTXOs
            match select_utxos(&utxos, amount) {
                Ok(selected) => {
                    println!("\nSelected {} UTXOs for transaction:", selected.len());
                    let mut selected_total = 0u64;
                    for utxo in &selected {
                        println!("  - {} ({} sats)", utxo.outpoint, utxo.value);
                        selected_total += utxo.value;
                    }
                    println!("Selected total: {} sats", selected_total);
                    
                    // Fetch Rune UTXOs
                    println!("\nFetching rune UTXOs from runes address...");
                    match fetch_rune_utxos(ord_server, runes_address) {
                        Ok(rune_utxos) => {
                            println!("Found {} UTXOs containing {}", rune_utxos.len(), RUNE_NAME);
                            
                            for utxo in &rune_utxos {
                                if let Some(rune_info) = utxo.runes.get(RUNE_NAME) {
                                    println!("  - {} ({} sats, {} {} runes)", 
                                        utxo.outpoint, 
                                        utxo.value, 
                                        rune_info.amount,
                                        rune_info.symbol
                                    );
                                }
                            }
                            
                            // Build the transaction
                            let network = parse_network(network);
                            match build_transaction(&selected, &rune_utxos, btc_address, destination_address, amount, network) {
                        Ok(tx) => {
                            println!("\nTransaction created successfully!");
                            println!("Transaction ID: {}", tx.compute_txid());
                            println!("Version: {}", tx.version);
                            println!("Inputs: {}", tx.input.len());
                            println!("Outputs: {}", tx.output.len());
                            
                            // Show output details
                            for (i, output) in tx.output.iter().enumerate() {
                                let desc = match i {
                                    0 => " (P2A anchor)",
                                    1 => " (destination)",
                                    2 => " (change)",
                                    _ => "",
                                };
                                println!("  Output {}: {} sats{}", i, output.value.to_sat(), desc);
                            }
                            
                            // Calculate fee
                            let total_inputs = selected_total + rune_utxos[0].value;
                            let total_outputs: u64 = tx.output.iter().map(|o| o.value.to_sat()).sum();
                            let fee = total_inputs - total_outputs;
                            println!("Total inputs: {} sats", total_inputs);
                            println!("Total outputs: {} sats", total_outputs);
                            println!("Fee: {} sats", fee);
                            
                            println!("\nRaw transaction hex:");
                            println!("{}", bitcoin::consensus::encode::serialize_hex(&tx));
                            
                            // Convert to PSBT
                            let psbt = match Psbt::from_unsigned_tx(tx) {
                                Ok(psbt) => psbt,
                                Err(e) => {
                                    eprintln!("Error creating PSBT: {}", e);
                                    std::process::exit(1);
                                }
                            };
                            
                            // Output PSBT in base64 format
                            println!("\nPSBT (base64):");
                            println!("{}", psbt.to_string());
                        }
                        Err(e) => {
                            eprintln!("Error building transaction: {}", e);
                            std::process::exit(1);
                        }
                    }
                        }
                        Err(e) => {
                            eprintln!("Error fetching rune UTXOs: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Error fetching UTXOs: {}", e);
            std::process::exit(1);
        }
    }
}