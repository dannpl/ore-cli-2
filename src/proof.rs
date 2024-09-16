use std::str::FromStr;

use ore_api::consts::TOKEN_DECIMALS;
use solana_program::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use spl_token::amount_to_ui_amount;

use crate::{ args::ProofArgs, utils::{ get_proof, proof_pubkey, get_config }, Miner };

impl Miner {
    pub async fn proof(&self, args: ProofArgs) {
        let signer = self.signer();

        let address = if let Some(address) = args.address {
            proof_pubkey(Pubkey::from_str(&address).unwrap())
        } else {
            proof_pubkey(signer.pubkey())
        };

        print!("Fetching proof for address {}... ", address);

        let proof = get_proof(&self.rpc_client, address).await;

        let config = get_config(&self.rpc_client).await;

        let multiplier = calculate_multiplier(proof.balance, config.top_balance);

        println!("Address: {:?}", address);
        println!("Authority: {:?}", proof.authority);
        println!("Multiplier {:?}", multiplier);
        println!("Balance: {:?} ORE", amount_to_ui_amount(proof.balance, TOKEN_DECIMALS));
        println!(
            "Last hash: {}",
            solana_sdk::hash::Hash::new_from_array(proof.last_hash).to_string()
        );
        println!("Last hash at: {:?}", proof.last_hash_at);
        println!("Last stake at: {:?}", proof.last_stake_at);
        println!("Miner: {:?}", proof.miner);
        println!("Total hashes: {:?}", proof.total_hashes);
        println!(
            "Total rewards: {:?} ORE",
            amount_to_ui_amount(proof.total_rewards, TOKEN_DECIMALS)
        );
    }
}

fn calculate_multiplier(balance: u64, top_balance: u64) -> f64 {
    1.0 + ((balance as f64) / (top_balance as f64)).min(1.0f64)
}
