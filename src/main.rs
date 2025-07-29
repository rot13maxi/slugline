use clap::{Parser, Subcommand, ValueEnum};

mod build_tx;
mod run_searcher;

#[derive(Debug, Clone, ValueEnum)]
enum Network {
    Regtest,
    Testnet4,
    Signet,
    Mainnet,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Bitcoin daemon host
    #[arg(long, default_value = "localhost")]
    bitcoind_host: String,

    /// Bitcoin daemon username
    #[arg(long)]
    bitcoind_user: Option<String>,

    /// Bitcoin daemon password
    #[arg(long)]
    bitcoind_password: Option<String>,

    /// Bitcoin network
    #[arg(long, value_enum, default_value = "mainnet")]
    network: Network,

    /// Ord server URL
    #[arg(long, default_value = "http://localhost")]
    ord_server: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Build a transaction
    BuildTx {
        /// Bitcoin address to use as input
        #[arg(long)]
        btc_address: String,
        
        /// Runes address
        #[arg(long)]
        runes_address: String,
        
        /// Destination address
        #[arg(long)]
        destination_address: String,
        
        /// Amount to send (in satoshis)
        #[arg(long)]
        amount: u64,
    },
    /// Run the searcher
    RunSearcher {
        /// Bitcoin Core wallet name to use
        #[arg(long, default_value = "searcher")]
        wallet: String,
        
        /// Fee rate in sat/vB for CPFP transactions
        #[arg(long, default_value = "100.0")]
        fee_rate: f64,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::BuildTx {
            btc_address,
            runes_address,
            destination_address,
            amount,
        } => {
            build_tx::run(
                &cli.bitcoind_host,
                cli.bitcoind_user.as_deref(),
                cli.bitcoind_password.as_deref(),
                &format!("{:?}", cli.network).to_lowercase(),
                &cli.ord_server,
                &btc_address,
                &runes_address,
                &destination_address,
                amount,
            );
        }
        Commands::RunSearcher { wallet, fee_rate } => {
            run_searcher::run(
                &cli.bitcoind_host,
                cli.bitcoind_user.as_deref(),
                cli.bitcoind_password.as_deref(),
                &format!("{:?}", cli.network).to_lowercase(),
                &cli.ord_server,
                &wallet,
                fee_rate,
            );
        }
    }
}