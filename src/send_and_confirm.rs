use std::str::FromStr;
use rand::seq::SliceRandom;
use solana_client::client_error::Result as ClientResult;
use solana_program::{ instruction::Instruction, pubkey::Pubkey, system_instruction::transfer };
use solana_rpc_client::spinner;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    signature::Signer,
    transaction::Transaction,
};

use crate::Miner;

const MAX_RETRIES: u32 = 5;

impl Miner {
    pub async fn send_and_confirm(&self, ixs: &[Instruction]) -> Result<(), String> {
        let progress_bar = spinner::new_progress_bar();
        let signer = self.signer();
        let client = self.rpc_client.clone();
        let mut send_client = self.rpc_client.clone();

        let jito_tip = *self.tip.read().unwrap();

        let mut final_ixs = vec![ComputeBudgetInstruction::set_compute_unit_limit(500_000)];

        if jito_tip > 0 {
            send_client = self.jito_client.clone();
            final_ixs.push(
                self
                    .get_tip_transfer_ix(signer.pubkey(), jito_tip)
                    .map_err(|e| format!("Failed to create tip transfer instruction: {}", e))?
            );
        }

        final_ixs.extend_from_slice(ixs);

        let (hash, _slot) = client
            .get_latest_blockhash_with_commitment(client.commitment()).await
            .map_err(|e| format!("Failed to get latest blockhash: {}", e))?;

        let mut tx = Transaction::new_with_payer(&final_ixs, Some(&signer.pubkey()));

        tx.sign(&[&signer], hash);

        progress_bar.set_message(format!("Submitting transaction..."));
        let mut retry_count = 0;

        loop {
            match send_client.send_transaction(&tx).await {
                Ok(signature) => {
                    println!("Transaction submitted successfully. Signature: {}", signature);
                    // Wait for confirmation
                    match
                        client.confirm_transaction_with_spinner(
                            &signature,
                            &hash,
                            client.commitment()
                        ).await
                    {
                        Ok(_) => {
                            println!("Mining transaction confirmed successfully");
                            return Ok(());
                        }
                        Err(e) => {
                            println!("Transaction failed to confirm: {}", e);
                        }
                    }
                }
                Err(e) => {
                    println!("Mining transaction failed: {}", e);
                }
            }

            retry_count += 1;
            if retry_count >= MAX_RETRIES {
                return Err(format!("Max retries exceeded"));
            }

            println!("Retrying... (Attempt {} of {})", retry_count + 1, MAX_RETRIES);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
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
