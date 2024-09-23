use std::str::FromStr;

use rand::seq::SliceRandom;
use solana_client::{ client_error::Result as ClientResult, rpc_config::RpcSendTransactionConfig };
use solana_program::{
    instruction::Instruction,
    native_token::lamports_to_sol,
    pubkey::Pubkey,
    system_instruction::transfer,
};
use solana_rpc_client::spinner;
use solana_sdk::{
    commitment_config::CommitmentLevel,
    compute_budget::ComputeBudgetInstruction,
    signature::{ Signature, Signer },
    transaction::Transaction,
};
use solana_transaction_status::UiTransactionEncoding;

use crate::Miner;

const RPC_RETRIES: usize = 0;

pub enum ComputeBudget {
    #[allow(dead_code)]
    Dynamic,
    Fixed(u32),
}

impl Miner {
    pub async fn send_and_confirm(
        &self,
        ixs: &[Instruction],
        compute_budget: ComputeBudget,
        _skip_confirm: bool
    ) -> ClientResult<Signature> {
        let progress_bar = spinner::new_progress_bar();
        let signer = self.signer();
        let fee_payer = self.fee_payer();
        let mut send_client = self.rpc_client.clone();

        let mut final_ixs = vec![];
        match compute_budget {
            ComputeBudget::Dynamic => {
                todo!("simulate tx");
            }
            ComputeBudget::Fixed(cus) => {
                final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(cus));
            }
        }

        final_ixs.extend_from_slice(ixs);

        let jito_tip = *self.tip.read().unwrap();
        if jito_tip > 0 {
            send_client = self.jito_client.clone();
        }
        if jito_tip > 0 {
            let tip_accounts = [
                "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
                "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
                "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
                "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
                "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
                "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
                "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
                "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
            ];
            final_ixs.push(
                transfer(
                    &signer.pubkey(),
                    &Pubkey::from_str(
                        &tip_accounts.choose(&mut rand::thread_rng()).unwrap().to_string()
                    ).unwrap(),
                    jito_tip
                )
            );
            progress_bar.println(format!("  Jito tip: {} SOL", lamports_to_sol(jito_tip)));
        }

        progress_bar.set_message(format!("Submitting transaction..."));

        let sig = send_client.send_transaction_with_config(
            &Transaction::new_with_payer(&final_ixs, Some(&fee_payer.pubkey())),
            RpcSendTransactionConfig {
                skip_preflight: true,
                preflight_commitment: Some(CommitmentLevel::Confirmed),
                encoding: Some(UiTransactionEncoding::Base64),
                max_retries: Some(RPC_RETRIES),
                min_context_slot: None,
            }
        ).await;

        sig
    }
}
