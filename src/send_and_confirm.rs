use std::str::FromStr;

use rand::seq::SliceRandom;
use solana_client::{ client_error::Result as ClientResult, rpc_config::RpcSendTransactionConfig };
use solana_program::{ instruction::Instruction, pubkey::Pubkey, system_instruction::transfer };
use solana_rpc_client::spinner;
use solana_sdk::{
    commitment_config::CommitmentLevel,
    compute_budget::ComputeBudgetInstruction,
    signature::{ Signature, Signer },
    transaction::Transaction,
};
use solana_transaction_status::UiTransactionEncoding;
use std::time::Duration;

const MAX_RETRIES: usize = 0;

use crate::Miner;

pub enum ComputeBudget {
    #[allow(dead_code)]
    Dynamic,
    Fixed(u32),
}

impl Miner {
    pub async fn send_and_confirm(
        &self,
        ixs: &[Instruction],
        compute_budget: ComputeBudget
    ) -> ClientResult<Signature> {
        let progress_bar = spinner::new_progress_bar();
        let signer = self.signer();
        let client = self.rpc_client.clone();
        let mut send_client = self.rpc_client.clone();

        let jito_tip = *self.tip.read().unwrap();

        let mut final_ixs = vec![self.get_compute_budget_ix(compute_budget)];

        if jito_tip > 0 {
            send_client = self.jito_client.clone();
            final_ixs.push(self.get_tip_transfer_ix(signer.pubkey(), jito_tip)?);
        }

        final_ixs.extend_from_slice(ixs);

        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Processed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(MAX_RETRIES),
            min_context_slot: None,
        };

        let mut tx = Transaction::new_with_payer(&final_ixs, Some(&signer.pubkey()));

        let (hash, _slot) = client.get_latest_blockhash_with_commitment(
            self.rpc_client.commitment()
        ).await?;
        tx.sign(&[&signer], hash);

        for attempt in 1..=MAX_RETRIES {
            progress_bar.set_message(format!("Submitting transaction (attempt {})...", attempt));
            match send_client.send_transaction_with_config(&tx, send_cfg).await {
                Ok(sig) => {
                    progress_bar.finish_with_message(format!("Sent: {}", sig));

                    return Ok(sig);
                }
                Err(err) => {
                    progress_bar.set_message(format!("ERROR (attempt {}): {}", attempt, err));
                    if attempt == MAX_RETRIES {
                        return Err(err);
                    }
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                }
            }
        }
        unreachable!()
    }

    fn get_compute_budget_ix(&self, compute_budget: ComputeBudget) -> Instruction {
        match compute_budget {
            ComputeBudget::Dynamic => ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
            ComputeBudget::Fixed(cus) => ComputeBudgetInstruction::set_compute_unit_limit(cus),
        }
    }

    fn get_tip_transfer_ix(&self, from: Pubkey, amount: u64) -> ClientResult<Instruction> {
        const TIPS: [&str; 8] = [
            "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
            "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
            "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
            "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
            "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
            "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
            "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
            "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
        ];

        let to = Pubkey::from_str(TIPS.choose(&mut rand::thread_rng()).unwrap());
        Ok(transfer(&from, &to.unwrap(), amount))
    }
}
