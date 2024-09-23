use std::str::FromStr;

use colored::*;
use indicatif::ProgressBar;
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

use crate::utils::get_latest_blockhash_with_retries;
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
        let client = self.rpc_client.clone();
        let fee_payer = self.fee_payer();
        let mut send_client = self.rpc_client.clone();

        // Set compute budget
        let mut final_ixs = vec![];
        match compute_budget {
            ComputeBudget::Dynamic => {
                todo!("simulate tx");
            }
            ComputeBudget::Fixed(cus) => {
                final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(cus));
            }
        }

        // Add in user instructions
        final_ixs.extend_from_slice(ixs);

        // Add jito tip
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

        // Build tx
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(RPC_RETRIES),
            min_context_slot: None,
        };
        let mut tx = Transaction::new_with_payer(&final_ixs, Some(&fee_payer.pubkey()));

        // Submit tx
        let mut attempts = 0;
        loop {
            progress_bar.set_message(format!("Submitting transaction... (attempt {})", attempts));

            // Sign tx with a new blockhash (after approximately ~45 sec)
            if attempts % 10 == 0 {
                // Resign the tx
                let (hash, _slot) = get_latest_blockhash_with_retries(&client).await?;
                if signer.pubkey() == fee_payer.pubkey() {
                    tx.sign(&[&signer], hash);
                } else {
                    tx.sign(&[&signer, &fee_payer], hash);
                }
            }

            // Send transaction
            attempts += 1;
            match send_client.send_transaction_with_config(&tx, send_cfg).await {
                Ok(sig) => {
                    progress_bar.finish_with_message(format!("Sent: {}", sig));
                    return Ok(sig);
                }

                // Handle submit errors
                Err(err) => {
                    log_error(&progress_bar, &err.kind().to_string(), false);
                }
            }
        }
    }
}

fn log_error(progress_bar: &ProgressBar, err: &str, finish: bool) {
    if finish {
        progress_bar.finish_with_message(format!("{} {}", "ERROR".bold().red(), err));
    } else {
        progress_bar.println(format!("  {} {}", "ERROR".bold().red(), err));
    }
}
