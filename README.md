# slugline

A carpool for Bitcoin Transactions. Pay your way with Runes!

Alice wants to make a Bitcoin transaction but doesn't want to pay transaction fees with Bitcoin. She wants to use a Rune to pay her fees. So she makes a transaction that pays zero fee. She attaches an input that contains a specific Rune. She also makes an output that is a keyless ephemeral anchor (p2a). She sends it to one or more `searchers`. Bob is a searcher. He sees the transaction and decides it's worth paying Alice's fee to claim the rune. He makes a transaction that CPFP Alice's transaction by spending the p2a output. This claims the rune and pulls Alice's transaction into a block.

While in theory a market can be made by any spenders and searchers that can agree on what Rune to use, I think it would make the most sense to have everyone standardize on the same rune to use for this. Probably something like UNCOMMON•GOODS because its easy to calculate the cost of minting that rune vs buying it, so slugline participants can calculate if it makes sense to do these kinds of deals.

Other options might be miners (or groups of miners) creating their own rune that is usable for block inclusion. This could have the effect of selling future blockspace in a tokenized way, and then redeeming that blockspace claim in-band.

The current implementation has the searcher run a web service. I think it would make more sense to plug into a broadcast network like NOSTR. Either I'll do that or if you want to make a PR, I'm thrilled to review it!

## Overview

slugline consists of two main components:

1. **build-tx**: Creates unsigned Bitcoin transactions (PSBTs) that use rune UTXOs for fee payment
2. **run-searcher**: A web service that accepts signed PSBTs and bumps their fees using CPFP

## Architecture

### Transaction Structure

Transactions built by slugline have a specific structure:
- **Inputs**: 
  - Regular Bitcoin UTXOs (for the payment amount)
  - One rune-containing UTXO (for fee payment) - always added as the last input
- **Outputs**:
  - First output: P2A (Pay-to-Anchor) with 0 sats - `OP_1 <0x4e73>`
  - Second output: Payment to destination
  - Third output (optional): Change back to sender (includes rune UTXO value)

All transactions are **version 3** for package relay support.

The P2A output serves as an anchor point that the searcher can spend to perform CPFP.

### Rune Support

The system is configured to work with `TESTSLUGLINERUNE` by default. This can be changed by modifying the `RUNE_NAME` constant in both `build_tx.rs` and `run_searcher.rs`.

## Installation

```bash
# Clone the repository
git clone https://github.com/yourusername/slugline
cd slugline

# Build the project
cargo build --release
```

## Dependencies

- Local Bitcoin node with RPC access (with wallet loaded for searcher)
- Local web service at `http://localhost/` that provides UTXO and transaction data
- Rust 1.70+ with cargo

## Usage

### Global Options

Both commands accept these options for connecting to bitcoind:

- `--bitcoind-host`: Bitcoin daemon host (default: localhost)
- `--bitcoind-user`: Bitcoin daemon RPC username
- `--bitcoind-password`: Bitcoin daemon RPC password
- `--network`: Bitcoin network - regtest, testnet4, signet, or mainnet (default: mainnet)

### Building Transactions

```bash
cargo run -- build-tx \
  --btc-address <BTC_ADDRESS> \
  --runes-address <RUNES_ADDRESS> \
  --destination-address <DESTINATION_ADDRESS> \
  --amount <AMOUNT_IN_SATS>
```

**Parameters:**
- `--btc-address`: Address containing regular Bitcoin UTXOs for payment
- `--runes-address`: Address containing rune UTXOs for fee payment
- `--destination-address`: Where to send the payment
- `--amount`: Amount to send in satoshis

**Example:**
```bash
cargo run -- build-tx \
  --network regtest \
  --btc-address "bcrt1qexample..." \
  --runes-address "bcrt1qrunes..." \
  --destination-address "bcrt1qdest..." \
  --amount 100000
```

**Output:**
- Transaction details (inputs, outputs, fees)
- Raw transaction hex
- **PSBT in base64 format** (ready for signing)

### Running the Searcher

```bash
cargo run -- run-searcher \
  --bitcoind-host localhost \
  --bitcoind-user myuser \
  --bitcoind-password mypass \
  --network regtest \
  --wallet mywallet
```

**Parameters:**
- `--wallet`: Bitcoin Core wallet name (default: "searcher")

The searcher automatically selects the correct RPC port based on the network:
- mainnet: 8332
- testnet: 18332
- signet: 38332
- regtest: 18443

This starts a web server on `http://127.0.0.1:3000` that accepts PSBTs for fee bumping.

**API Endpoint:**
- `POST /submit-psbt`
- Content-Type: `application/json`
- Body: `{"psbt": "<base64_encoded_psbt>"}`

**Example request:**
```bash
curl -X POST http://127.0.0.1:3000/submit-psbt \
  -H "Content-Type: application/json" \
  -d '{"psbt": "cHNidP8BAH0CAAAAAeFH5Kf..."}'
```

**Response:**
```json
{
  "success": true,
  "message": "Package submitted successfully",
  "package_txids": [
    "abcd1234...",  // Parent transaction ID
    "efgh5678..."   // CPFP transaction ID
  ]
}
```

## How It Works

### Transaction Building Process

1. **UTXO Selection**: 
   - Fetches UTXOs from the BTC address
   - Selects enough UTXOs to cover the payment amount (largest first)
   - Fetches rune-containing UTXOs from the runes address

2. **Transaction Construction**:
   - Adds selected BTC UTXOs as inputs
   - Adds one rune UTXO as the last input
   - Creates P2A output (0 sats) as first output
   - Creates payment output
   - Creates change output if needed (includes rune UTXO value)
   - Sets transaction version to 3

3. **PSBT Generation**:
   - Converts the unsigned transaction to PSBT format
   - Outputs base64-encoded PSBT for signing

### Searcher Operation

1. **Validation**:
   - Decodes the submitted PSBT
   - Verifies first output is P2A (`OP_1 <0x4e73>`) with 0 sats
   - Verifies last input contains the required rune

2. **CPFP Transaction**:
   - Creates a version 3 child transaction with:
     - Input 1: The P2A output from the parent
     - Input 2: One of the searcher's own UTXOs
   - Single output returning funds to searcher minus fees
   - Fee calculation: `(parent_vsize + child_vsize) * fee_rate`

3. **Transaction Signing**:
   - Signs the CPFP transaction using `signrawtransactionwithwallet`
   - Provides the P2A output details via `prevtxs` parameter

4. **Package Submission**:
   - Submits both parent and child transactions as a package
   - Uses Bitcoin Core's `submitpackage` RPC
   - Properly handles error responses (checks `package_msg` field)

## Local Web Service API

The system expects a local web service at `http://localhost/` with these endpoints:

### GET /outputs/{address}
Returns UTXOs for an address:
```json
[
  {
    "address": "bc1q...",
    "confirmations": 2,
    "outpoint": "txid:vout",
    "runes": {
      "TESTSLUGLINERUNE": {
        "amount": 10000,
        "divisibility": 2,
        "symbol": "$"
      }
    },
    "value": 10000,
    "spent": false,
    "script_pubkey": "0014...",
    "transaction": "...",
    "indexed": true,
    "inscriptions": [],
    "sat_ranges": null
  }
]
```

### GET /tx/{txid}
Returns transaction details with outputs containing `script_pubkey` field (used by searcher for validation).

## Technical Details

### P2A Script
The Pay-to-Anchor script is exactly: `OP_1 <0x4e73>`
- Script hex: `51024e73`
- This creates an anyone-can-spend output that can be used for CPFP

### Version 3 Transactions
Both parent and child transactions use version 3 (`0x03000000`) for package relay support.

### Fee Calculation
The searcher calculates fees for both transactions:
```
total_vsize = parent_vsize + child_vsize
total_fee = total_vsize * fee_rate
```

## Security Considerations

1. **Private Keys**: This tool only creates unsigned transactions. Private keys are never handled.
2. **RPC Security**: Use proper authentication for Bitcoin Core RPC access.
3. **Network Security**: The searcher binds to localhost only by default.
4. **Wallet Security**: Ensure the searcher wallet has sufficient UTXOs and is properly secured.

## Fee Structure

- Transaction fees are paid from the rune UTXO's bitcoin value
- The searcher calculates appropriate fees for both parent and child transactions
- Fee rate is currently hardcoded to 100 sat/vB in the searcher
- Change calculation includes the rune UTXO value to avoid dust errors

## Troubleshooting

### Common Issues

1. **"Connection refused" errors**: 
   - Ensure Bitcoin Core is running
   - Check the network matches your Bitcoin Core configuration
   - Verify RPC credentials

2. **"No UTXOs available in searcher wallet"**:
   - Fund the searcher wallet with some Bitcoin
   - Ensure the wallet is loaded in Bitcoin Core

3. **"Input not found or already spent" during signing**:
   - This is normal - the P2A output isn't on-chain yet
   - The searcher provides the output details via `prevtxs`

4. **Package submission failures**:
   - Check Bitcoin Core logs for detailed error messages
   - Ensure mempool accepts version 3 transactions
   - Verify fee rates are sufficient

## Limitations

- Currently supports only one rune UTXO per transaction
- Fee estimation is basic (no dynamic fee adjustment)
- Searcher uses only the first available UTXO from its wallet
- No support for RBF beyond the sequence number setting

## Future Improvements

- Rune Change (right now the spender sends ALL the runes in an input, they should get change)
- Support for multiple rune UTXOs
- Dynamic fee estimation
- Better UTXO selection for searcher


## License

MIT
