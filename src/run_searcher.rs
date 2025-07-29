use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::post,
    Router,
};
use bitcoin::{
    absolute,
    opcodes::all::OP_PUSHNUM_1,
    psbt::Psbt,
    script::{Builder, PushBytesBuf},
    transaction::{OutPoint, Transaction, TxIn, TxOut},
    Amount, Network, ScriptBuf, Sequence, Witness,
};
use bitcoincore_rpc::{Auth, Client, RpcApi, json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

// Module-level constant for the rune we're working with
const RUNE_NAME: &str = "TESTSLUGLINERUNE";

#[derive(Debug, Clone)]
struct AppState {
    bitcoind_host: String,
    bitcoind_user: Option<String>,
    bitcoind_password: Option<String>,
    network: Network,
    wallet_name: String,
    fee_rate: f64,
    ord_server: String,
}

#[derive(Debug, Deserialize)]
struct SubmitPsbtRequest {
    psbt: String,
}

#[derive(Debug, Serialize)]
struct SubmitPsbtResponse {
    success: bool,
    message: String,
    package_txids: Option<Vec<String>>,
}

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

fn create_p2a_script() -> ScriptBuf {
    let push_bytes = PushBytesBuf::try_from(&[0x4e, 0x73]).unwrap();
    Builder::new()
        .push_opcode(OP_PUSHNUM_1)
        .push_slice(push_bytes)
        .into_script()
}

fn validate_transaction(tx: &Transaction) -> Result<(), String> {
    // Check first output is P2A
    if tx.output.is_empty() {
        return Err("Transaction has no outputs".to_string());
    }
    
    let expected_p2a = create_p2a_script();
    if tx.output[0].script_pubkey != expected_p2a {
        return Err("First output is not a P2A output".to_string());
    }
    
    if tx.output[0].value != Amount::ZERO {
        return Err("P2A output value is not 0".to_string());
    }
    
    Ok(())
}

async fn fetch_utxo_info(outpoint: &OutPoint, network: Network, ord_server: &str) -> Result<Utxo, Box<dyn Error + Send + Sync>> {
    // First, fetch the transaction to get the output script
    let url = format!("{}/tx/{}", ord_server, outpoint.txid);
    info!("Fetching transaction details from: {}", url);
    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await?;
    
    if !response.status().is_success() {
        return Err(format!("Failed to fetch transaction: {}", response.status()).into());
    }
    
    let tx_data: serde_json::Value = response.json().await?;
    
    // Get the output at the specified vout
    let outputs = tx_data["transaction"]["output"]
        .as_array()
        .ok_or("No outputs in transaction")?;
    
    let output = outputs.get(outpoint.vout as usize)
        .ok_or("Output index out of bounds")?;
    
    // Get the script_pubkey and convert to address
    let script_hex = output["script_pubkey"]
        .as_str()
        .ok_or("No script_pubkey in output")?;
    
    let script_bytes = hex::decode(script_hex)
        .map_err(|e| format!("Failed to decode script hex: {}", e))?;
    
    let script = ScriptBuf::from_bytes(script_bytes);
    
    // Try to extract address from script
    let address = bitcoin::Address::from_script(&script, network)
        .map_err(|e| format!("Failed to derive address from script: {}", e))?;
    
    // Now fetch the UTXO info for this specific output
    let url = format!("{}/outputs/{}", ord_server, address);
    info!("Fetching UTXO info from: {}", url);
    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await?;
    
    if !response.status().is_success() {
        return Err(format!("Failed to fetch UTXOs: {}", response.status()).into());
    }
    
    let utxos: Vec<Utxo> = response.json().await?;
    
    // Find the specific UTXO matching our outpoint
    let outpoint_str = format!("{}:{}", outpoint.txid, outpoint.vout);
    utxos.into_iter()
        .find(|u| u.outpoint == outpoint_str)
        .ok_or_else(|| format!("UTXO not found for outpoint: {}", outpoint_str).into())
}

async fn validate_rune_input(tx: &Transaction, network: Network, ord_server: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    if tx.input.is_empty() {
        return Err("Transaction has no inputs".into());
    }
    
    // Check the last input for runes
    let last_input = &tx.input[tx.input.len() - 1];
    let utxo_info = fetch_utxo_info(&last_input.previous_output, network, ord_server).await?;
    
    if !utxo_info.runes.contains_key(RUNE_NAME) {
        return Err(format!("Last input does not contain {} rune", RUNE_NAME).into());
    }
    
    Ok(())
}

fn create_cpfp_transaction(
    parent_tx: &Transaction,
    searcher_utxo: &json::ListUnspentResultEntry,
    fee_rate: f64,
) -> Result<Transaction, Box<dyn Error>> {
    let mut inputs = Vec::new();
    
    // Input 1: P2A output from parent transaction (first output)
    let parent_txid = parent_tx.compute_txid();
    inputs.push(TxIn {
        previous_output: OutPoint {
            txid: parent_txid,
            vout: 0, // P2A is always first output
        },
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::default(),
    });
    
    // Input 2: Searcher's UTXO
    inputs.push(TxIn {
        previous_output: OutPoint {
            txid: searcher_utxo.txid,
            vout: searcher_utxo.vout,
        },
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::default(),
    });
    
    // Build a dummy child transaction to get accurate size
    let dummy_output = TxOut {
        value: Amount::from_sat(searcher_utxo.amount.to_sat()),
        script_pubkey: searcher_utxo.address.as_ref()
            .ok_or("No address in UTXO")?
            .clone()
            .assume_checked()
            .script_pubkey(),
    };
    
    let dummy_tx = Transaction {
        version: bitcoin::transaction::Version(3),
        lock_time: absolute::LockTime::ZERO,
        input: inputs.clone(),
        output: vec![dummy_output],
    };
    
    // Calculate virtual sizes (weight / 4)
    let parent_weight = parent_tx.weight().to_wu();
    let child_weight = dummy_tx.weight().to_wu();
    let parent_vsize = (parent_weight + 3) / 4; // Round up
    let child_vsize = (child_weight + 3) / 4; // Round up
    
    info!("Parent transaction vsize: {} vbytes", parent_vsize);
    info!("Child transaction vsize: {} vbytes", child_vsize);
    
    // Calculate total fee needed for both transactions
    let total_vsize = parent_vsize + child_vsize;
    let total_fee = (total_vsize as f64 * fee_rate).ceil() as u64;
    
    info!("Total vsize: {} vbytes, Fee rate: {} sat/vB, Total fee: {} sats", 
          total_vsize, fee_rate, total_fee);
    
    // Output: Return searcher's funds minus total fees
    let output_value = searcher_utxo.amount.to_sat().saturating_sub(total_fee);
    
    let outputs = vec![TxOut {
        value: Amount::from_sat(output_value),
        script_pubkey: searcher_utxo.address.as_ref()
            .ok_or("No address in UTXO")?
            .clone()
            .assume_checked()
            .script_pubkey(),
    }];
    
    Ok(Transaction {
        version: bitcoin::transaction::Version(3),
        lock_time: absolute::LockTime::ZERO,
        input: inputs,
        output: outputs,
    })
}

async fn handle_submit_psbt(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SubmitPsbtRequest>,
) -> Result<Json<SubmitPsbtResponse>, StatusCode> {
    info!("Received PSBT submission");
    
    // Parse PSBT
    let psbt = match Psbt::from_str(&payload.psbt) {
        Ok(psbt) => {
            info!("Successfully parsed PSBT");
            psbt
        },
        Err(e) => {
            error!("Failed to parse PSBT: {}", e);
            return Ok(Json(SubmitPsbtResponse {
                success: false,
                message: format!("Invalid PSBT: {}", e),
                package_txids: None,
            }));
        }
    };

    
    let tx = psbt.extract_tx().expect("Failed to extract transaction from PSBT");
    info!("Transaction has {} inputs and {} outputs", tx.input.len(), tx.output.len());
    
    // Validate P2A output
    info!("Validating P2A output...");
    if let Err(e) = validate_transaction(&tx) {
        error!("P2A validation failed: {}", e);
        return Ok(Json(SubmitPsbtResponse {
            success: false,
            message: e,
            package_txids: None,
        }));
    }
    info!("P2A output validation passed");
    
    // Validate rune input
    info!("Validating rune input...");
    if let Err(e) = validate_rune_input(&tx, state.network, &state.ord_server).await {
        error!("Rune validation failed: {}", e);
        return Ok(Json(SubmitPsbtResponse {
            success: false,
            message: format!("Rune validation failed: {}", e),
            package_txids: None,
        }));
    }
    info!("Rune input validation passed");
    
    // Connect to Bitcoin Core
    let auth = match (&state.bitcoind_user, &state.bitcoind_password) {
        (Some(user), Some(pass)) => {
            info!("Using RPC auth with user: {}", user);
            Auth::UserPass(user.clone(), pass.clone())
        },
        _ => {
            info!("Using RPC with no auth");
            Auth::None
        },
    };
    
    // Select RPC port based on network
    let rpc_port = match state.network {
        Network::Bitcoin => 8332,
        Network::Testnet => 18332,
        Network::Signet => 38332,
        Network::Regtest => 18443,
        _ => 8332, // Default to mainnet port
    };
    
    let rpc_url = format!("http://{}:{}/wallet/{}", state.bitcoind_host, rpc_port, state.wallet_name);
    info!("Connecting to Bitcoin Core RPC at: {} (network: {:?})", rpc_url, state.network);
    
    let client = match Client::new(&rpc_url, auth) {
        Ok(client) => {
            info!("Successfully connected to Bitcoin Core");
            client
        },
        Err(e) => {
            error!("Failed to connect to Bitcoin Core at {}: {}", rpc_url, e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    
    // Get searcher's wallet UTXOs
    info!("Fetching searcher's wallet UTXOs...");
    let unspent = match client.list_unspent(Some(1), None, None, None, None) {
        Ok(unspent) => {
            info!("Found {} unspent UTXOs in searcher wallet", unspent.len());
            unspent
        },
        Err(e) => {
            error!("Failed to list unspent: {:?}", e);
            error!("Make sure Bitcoin Core is running and the wallet is loaded");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    
    if unspent.is_empty() {
        return Ok(Json(SubmitPsbtResponse {
            success: false,
            message: "No UTXOs available in searcher wallet".to_string(),
            package_txids: None,
        }));
    }
    
    // Use the first available UTXO
    let searcher_utxo = &unspent[0];
    
    // Create CPFP transaction
    let cpfp_tx = match create_cpfp_transaction(&tx, searcher_utxo, state.fee_rate) {
        Ok(tx) => tx,
        Err(e) => {
            return Ok(Json(SubmitPsbtResponse {
                success: false,
                message: format!("Failed to create CPFP transaction: {}", e),
                package_txids: None,
            }));
        }
    };
    
    // Log CPFP transaction details
    info!("CPFP transaction has {} inputs:", cpfp_tx.input.len());
    for (i, input) in cpfp_tx.input.iter().enumerate() {
        info!("  Input {}: {}:{}", i, input.previous_output.txid, input.previous_output.vout);
    }
    
    // Get parent transaction ID for signing
    let parent_txid = tx.compute_txid();
    
    // Convert parent transaction to hex
    let parent_hex = bitcoin::consensus::encode::serialize_hex(&tx);
    info!("Parent transaction hex: {}", parent_hex);
    
    // Sign the CPFP transaction
    info!("Signing CPFP transaction with wallet...");
    
    // We need to provide the P2A output details since it's not on-chain yet
    let p2a_script = create_p2a_script();
    let p2a_script_hex = bitcoin::consensus::encode::serialize_hex(&p2a_script);
    
    info!("P2A script for signing: {}", p2a_script_hex);
    info!("Parent txid: {}", parent_txid);
    
    let prev_tx_input = json::SignRawTransactionInput {
        txid: parent_txid,
        vout: 0, // P2A is always first output
        script_pub_key: p2a_script,
        redeem_script: None,
        amount: Some(bitcoin::Amount::from_sat(0)), // P2A has 0 value
    };
    
    let prevtxs = vec![prev_tx_input];
    
    // The sign_raw_transaction_with_wallet method expects the transaction itself, not hex
    let sign_result = match client.sign_raw_transaction_with_wallet(&cpfp_tx, Some(&prevtxs), None) {
        Ok(result) => result,
        Err(e) => {
            error!("Failed to sign CPFP transaction: {:?}", e);
            return Ok(Json(SubmitPsbtResponse {
                success: false,
                message: format!("Failed to sign CPFP transaction: {}", e),
                package_txids: None,
            }));
        }
    };
    
    if !sign_result.complete {
        error!("Failed to fully sign CPFP transaction");
        if let Some(errors) = &sign_result.errors {
            for error in errors {
                error!("Signing error: {:?}", error);
            }
        }
        return Ok(Json(SubmitPsbtResponse {
            success: false,
            message: "Failed to fully sign CPFP transaction".to_string(),
            package_txids: None,
        }));
    }
    
    // Convert the signed transaction result to hex string
    let child_hex = hex::encode(&sign_result.hex);
    info!("Signed child transaction hex: {}", child_hex);
    
    // Submit package
    let package = vec![parent_hex, child_hex];
    
    match client.call::<serde_json::Value>("submitpackage", &[serde_json::json!(package)]) {
        Ok(result) => {
            info!("Package submission response: {:?}", result);
            
            // Check if the response indicates an error
            if let Some(package_msg) = result.get("package_msg") {
                if package_msg == "transaction failed" {
                    // Extract error details
                    let mut error_details = Vec::new();
                    
                    if let Some(tx_results) = result.get("tx-results").and_then(|v| v.as_object()) {
                        for (txid, tx_result) in tx_results {
                            if let Some(error) = tx_result.get("error").and_then(|v| v.as_str()) {
                                error_details.push(format!("{}: {}", txid, error));
                            }
                        }
                    }
                    
                    let error_msg = if error_details.is_empty() {
                        "Package submission failed with unknown error".to_string()
                    } else {
                        format!("Package submission failed: {}", error_details.join(", "))
                    };
                    
                    error!("{}", error_msg);
                    return Ok(Json(SubmitPsbtResponse {
                        success: false,
                        message: error_msg,
                        package_txids: None,
                    }));
                }
            }
            
            // Success case
            let txids = vec![
                tx.compute_txid().to_string(),
                cpfp_tx.compute_txid().to_string(),
            ];
            
            Ok(Json(SubmitPsbtResponse {
                success: true,
                message: "Package submitted successfully".to_string(),
                package_txids: Some(txids),
            }))
        }
        Err(e) => {
            error!("Failed to submit package: {}", e);
            Ok(Json(SubmitPsbtResponse {
                success: false,
                message: format!("Failed to submit package: {}", e),
                package_txids: None,
            }))
        }
    }
}

pub fn run(
    bitcoind_host: &str,
    bitcoind_user: Option<&str>,
    bitcoind_password: Option<&str>,
    network: &str,
    ord_server: &str,
    wallet_name: &str,
    fee_rate: f64,
) {
    // Initialize tracing
    tracing_subscriber::fmt::init();
    
    info!("Starting slugline searcher...");
    info!("Configuration:");
    info!("  Bitcoin host: {}", bitcoind_host);
    info!("  Bitcoin user: {}", bitcoind_user.unwrap_or("<none>"));
    info!("  Network: {}", network);
    info!("  Wallet: {}", wallet_name);
    info!("  Rune: {}", RUNE_NAME);
    info!("  Fee rate: {} sat/vB", fee_rate);
    
    let state = Arc::new(AppState {
        bitcoind_host: bitcoind_host.to_string(),
        bitcoind_user: bitcoind_user.map(String::from),
        bitcoind_password: bitcoind_password.map(String::from),
        network: parse_network(network),
        wallet_name: wallet_name.to_string(),
        fee_rate,
        ord_server: ord_server.to_string(),
    });
    
    // Build the runtime
    let runtime = tokio::runtime::Runtime::new().unwrap();
    
    runtime.block_on(async {
        // Create router
        let app = Router::new()
            .route("/submit-psbt", post(handle_submit_psbt))
            .layer(tower_http::trace::TraceLayer::new_for_http())
            .with_state(state);
        
        // Bind to address
        let addr = "127.0.0.1:3000";
        info!("Searcher listening on {}", addr);
        
        let listener = TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, app).await.unwrap();
    });
}