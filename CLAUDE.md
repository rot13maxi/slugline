# slugline - Implementation Details

This document contains detailed implementation notes for the slugline project, a Bitcoin transaction builder and searcher for rune-based fee payment systems.

## Project Structure

```
slugline/
├── Cargo.toml
├── README.md
├── CLAUDE.md (this file)
└── src/
    ├── main.rs         # CLI entry point with clap configuration
    ├── build_tx.rs     # Transaction building logic
    └── run_searcher.rs # Searcher web service
```

## Key Implementation Details

### Constants

- **Rune Name**: `TESTSLUGLINERUNE` (hardcoded in both modules)
- **P2A Script**: `OP_1 <0x4e73>` (hex: `51024e73`)
- **CPFP Fee Rate**: 100 sat/vB (hardcoded in searcher)
- **Searcher Port**: 3000
- **Transaction Version**: 3 (for package relay)

### Network Configuration

The system automatically selects the correct RPC port based on network:
- mainnet: 8332
- testnet: 18332  
- signet: 38332
- regtest: 18443

### Transaction Building (`build_tx.rs`)

1. **UTXO Selection Algorithm**:
   - Sorts UTXOs by value (descending)
   - Selects largest UTXOs first until target amount is reached
   - Returns error if insufficient funds

2. **Transaction Structure**:
   - Regular UTXOs added first
   - Rune UTXO always added as last input
   - P2A output always first (0 sats)
   - Payment output second
   - Change output third (if needed)

3. **Change Calculation**:
   ```rust
   let total_input = btc_input + rune_utxo.value;
   let change = total_input.saturating_sub(amount);
   ```

### Searcher Service (`run_searcher.rs`)

1. **Validation Steps**:
   - Decode PSBT
   - Verify P2A output (first output, 0 sats, correct script)
   - Verify rune input (last input contains required rune)

2. **CPFP Transaction Creation**:
   - Input 1: P2A output from parent (vout=0)
   - Input 2: First available searcher UTXO
   - Single output: Return to searcher minus fees

3. **Fee Calculation**:
   ```rust
   let parent_vsize = (parent_weight + 3) / 4; // Round up
   let child_vsize = (child_weight + 3) / 4;   // Round up
   let total_vsize = parent_vsize + child_vsize;
   let total_fee = (total_vsize as f64 * fee_rate).ceil() as u64;
   ```

4. **Transaction Signing**:
   - Uses `signrawtransactionwithwallet` RPC
   - Provides P2A output details via `prevtxs`:
   ```rust
   let prev_tx_input = json::SignRawTransactionInput {
       txid: parent_txid,
       vout: 0,
       script_pub_key: p2a_script,
       redeem_script: None,
       amount: Some(bitcoin::Amount::from_sat(0)),
   };
   ```

5. **Package Submission**:
   - Submits `[parent_hex, child_hex]` to `submitpackage`
   - Checks `package_msg` field for "transaction failed"
   - Extracts error details from `tx-results` if failed

### API Integration

The system expects a local web service with:

1. **GET /outputs/{address}**:
   - Returns array of UTXO objects
   - Filters by `spent: false`
   - Includes `runes` field for rune detection

2. **GET /tx/{txid}**:
   - Returns transaction details
   - Used to derive address from `script_pubkey`
   - Required for UTXO validation in searcher

### Error Handling

Common errors and their handling:

1. **RPC Connection**: Includes host, port, and network in error messages
2. **UTXO Selection**: Shows available vs required amounts
3. **Package Submission**: Parses and displays specific transaction errors
4. **Signing Failures**: Logs all signing errors from Bitcoin Core

### Logging

The searcher uses `tracing` for structured logging:
- Configuration details at startup
- Validation steps with pass/fail
- Transaction details (inputs, outputs, fees)
- RPC communication details
- Error details with context

## Development Notes

### Testing Commands

1. **Build a transaction**:
   ```bash
   cargo run -- --network regtest build-tx \
     --btc-address "bcrt1q..." \
     --runes-address "bcrt1q..." \
     --destination-address "bcrt1q..." \
     --amount 100000
   ```

2. **Run the searcher**:
   ```bash
   cargo run -- --network regtest run-searcher \
     --bitcoind-user user \
     --bitcoind-password pass \
     --wallet searcher
   ```

3. **Submit a PSBT**:
   ```bash
   curl -X POST http://127.0.0.1:3000/submit-psbt \
     -H "Content-Type: application/json" \
     -d '{"psbt": "cHNidP8..."}'
   ```

### Important Considerations

1. **Version 3 Transactions**: Required for package relay, use `bitcoin::transaction::Version(3)`

2. **Sequence Numbers**: Set to `ENABLE_RBF_NO_LOCKTIME` for RBF support

3. **Wallet Requirements**: 
   - Searcher needs loaded wallet with UTXOs
   - Wallet name included in RPC URL path

4. **UTXO Ordering**: Rune UTXO must be last input for proper rune transfer semantics

5. **P2A Script**: Must be exactly `OP_1 <0x4e73>` for compatibility

### Future Improvements

1. **Dynamic Fee Estimation**: Query mempool for appropriate fee rates
2. **UTXO Optimization**: Better selection algorithm (coin selection)
3. **Batch Processing**: Handle multiple PSBTs in parallel
4. **Configuration File**: Move hardcoded values to config
5. **Monitoring**: Add metrics for package success rates
6. **Rune Flexibility**: Support multiple rune types dynamically

## Debugging Tips

1. **Enable Bitcoin Core debug logging**:
   ```
   bitcoind -debug=rpc -debug=mempool
   ```

2. **Check searcher logs**: All operations are logged with context

3. **Verify transaction structure**:
   ```bash
   bitcoin-cli decoderawtransaction <hex>
   ```

4. **Test package locally**:
   ```bash
   bitcoin-cli testmempoolaccept '["parent_hex", "child_hex"]'
   ```

## Dependencies

Key crates and their purposes:
- `bitcoin`: Transaction construction and PSBT handling
- `bitcoincore-rpc`: Bitcoin Core RPC communication
- `axum`: Web framework for searcher service
- `clap`: CLI argument parsing (derive style)
- `reqwest`: HTTP client for UTXO API
- `tokio`: Async runtime for web server
- `tracing`: Structured logging