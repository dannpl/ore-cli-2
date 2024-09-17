#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reqwest")] Reqwest(#[from] reqwest::Error),
    #[error("solana rpc client")] SolanaRpcClient(#[from] solana_client::client_error::ClientError),
    #[error("solana program")] SolanaProgram(#[from] solana_program::program_error::ProgramError),
}
