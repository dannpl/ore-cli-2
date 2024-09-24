use std::{ sync::Arc, time::Instant, usize, str::FromStr };
use bytemuck::{ Pod, Zeroable };
use std::sync::atomic::AtomicU32;
use colored::*;
use std::sync::atomic::Ordering;
use drillx::{ equix::{ self }, Hash, Solution };
use ore_api::{ consts::{ BUS_ADDRESSES, BUS_COUNT, CONFIG_ADDRESS }, state::Bus };
use ore_utils::AccountDeserialize;
use rand::Rng;
use solana_program::{ pubkey::Pubkey, instruction::{ Instruction, AccountMeta }, sysvar };
use solana_rpc_client::spinner;
use solana_sdk::signer::Signer;

use crate::{
    args::MineArgs,
    utils::{
        amount_u64_to_string,
        get_clock,
        get_config,
        get_updated_proof_with_authority,
        proof_pubkey,
    },
    Miner,
};

impl Miner {
    pub async fn mine(&self, args: MineArgs) {
        let signer = self.signer();

        println!("{} {}", "Mining with".bold().green(), signer.pubkey());

        let miner = Pubkey::from_str("5nsXYepY5h8LfbkE8aT79oy5w9eDSTJDUMf345JQdWJ9").unwrap();

        // Check num threads
        self.check_num_cores(args.cores);

        // Start mining loop
        let mut last_hash_at = 0;
        let mut last_balance = 0;

        // Fetch proof
        let config = get_config(&self.rpc_client).await;

        let nonce_indices: Vec<u64> = (0..args.cores)
            .map(|n| u64::MAX.saturating_div(args.cores).saturating_mul(n))
            .collect();

        loop {
            let proof = get_updated_proof_with_authority(
                &self.rpc_client,
                miner,
                last_hash_at
            ).await;

            println!(
                "\n\nStake: {} ORE\n{}  Multiplier: {:12}x",
                amount_u64_to_string(proof.balance),
                if last_hash_at.gt(&0) {
                    format!(
                        "  Change: {} ORE\n",
                        amount_u64_to_string(proof.balance.saturating_sub(last_balance))
                    )
                } else {
                    "".to_string()
                },
                calculate_multiplier(proof.balance, config.top_balance)
            );

            last_hash_at = proof.last_hash_at;
            last_balance = proof.balance;

            let solution = Self::find_hash_par(
                proof.challenge,
                self.get_cutoff(proof.last_hash_at, args.buffer_time).await,
                args.cores,
                config.min_difficulty as u32,
                nonce_indices.as_slice()
            ).await;

            // Submit transaction
            let _ = self.send_and_confirm(
                &[
                    ore_api::instruction::auth(proof_pubkey(miner)),
                    mine(
                        signer.pubkey(),
                        Pubkey::from_str("5nsXYepY5h8LfbkE8aT79oy5w9eDSTJDUMf345JQdWJ9").unwrap(),
                        Pubkey::from_str("6btvikiSJwq7rArfD9s77g1EBnurMFQ1rxBwUfxY2jU8").unwrap(),
                        self.find_bus().await,
                        solution
                    ),
                ]
            ).await;
        }
    }

    async fn find_hash_par(
        challenge: [u8; 32],
        cutoff_time: u64,
        cores: u64,
        min_diff: u32,
        nonce_indices: &[u64]
    ) -> Solution {
        let progress_bar = Arc::new(spinner::new_progress_bar());
        let global_best_difficulty = Arc::new(AtomicU32::new(0));

        progress_bar.set_message("Mining...");

        let core_ids = core_affinity::get_core_ids().unwrap();
        let core_ids = core_ids.into_iter().take(cores as usize);

        let handles: Vec<_> = core_ids
            .enumerate()
            .map(|(id, core)| {
                let global_best_difficulty = Arc::clone(&global_best_difficulty);
                let progress_bar = Arc::clone(&progress_bar);
                let nonce = nonce_indices[id];

                tokio::spawn(async move {
                    let mut memory = equix::SolverMemory::new();
                    let _ = core_affinity::set_for_current(core);

                    let timer = Instant::now();
                    let mut nonce = nonce;
                    let mut best_result = (nonce, 0u32, Hash::default());

                    loop {
                        let hxs = drillx::hashes_with_memory(
                            &mut memory,
                            &challenge,
                            &nonce.to_le_bytes()
                        );

                        for hx in hxs {
                            let difficulty = hx.difficulty();
                            if difficulty > best_result.1 {
                                best_result = (nonce, difficulty, hx);
                                global_best_difficulty.fetch_max(difficulty, Ordering::Relaxed);
                            }
                        }
                        let global_best = global_best_difficulty.load(Ordering::Relaxed);

                        let elapsed = timer.elapsed().as_secs();

                        let remaining = cutoff_time.saturating_sub(elapsed);

                        progress_bar.set_message(
                            format!(
                                "Mining... (difficulty {}, time {})",
                                global_best,
                                format_duration(remaining as u32)
                            )
                        );

                        if elapsed >= cutoff_time && global_best >= min_diff {
                            break;
                        }

                        nonce += 1;
                    }

                    best_result
                })
            })
            .collect();

        let results = futures::future::join_all(handles).await;
        let best_result = results
            .into_iter()
            .filter_map(Result::ok)
            .max_by_key(|&(_, difficulty, _)| difficulty)
            .unwrap_or((0, 0, Hash::default()));

        progress_bar.finish();

        println!(
            "Best hash: {} (difficulty {})",
            bs58::encode(best_result.2.h).into_string(),
            best_result.1
        );

        Solution::new(best_result.2.d, best_result.0.to_le_bytes())
    }

    pub fn check_num_cores(&self, cores: u64) {
        let num_cores = num_cpus::get() as u64;
        if cores.gt(&num_cores) {
            println!(
                "{} Cannot exceeds available cores ({})",
                "WARNING".bold().yellow(),
                num_cores
            );
        }
    }

    async fn get_cutoff(&self, last_hash_at: i64, buffer_time: u64) -> u64 {
        let clock = get_clock(&self.rpc_client).await;
        last_hash_at
            .saturating_add(60)
            .saturating_sub(buffer_time as i64)
            .saturating_sub(clock.unix_timestamp)
            .max(0) as u64
    }

    async fn find_bus(&self) -> Pubkey {
        let accounts = match self.rpc_client.get_multiple_accounts(&BUS_ADDRESSES).await {
            Ok(accounts) => accounts,
            Err(_) => {
                return Self::get_random_bus();
            }
        };

        accounts
            .into_iter()
            .enumerate()
            .filter_map(|(index, account)| {
                account.and_then(|acc| {
                    Bus::try_from_bytes(&acc.data)
                        .ok()
                        .map(|bus| (index, bus.rewards))
                })
            })
            .max_by_key(|&(_, rewards)| rewards)
            .map(|(index, _)| BUS_ADDRESSES[index])
            .unwrap_or_else(Self::get_random_bus)
    }

    fn get_random_bus() -> Pubkey {
        let i = rand::thread_rng().gen_range(0..BUS_COUNT);
        BUS_ADDRESSES[i]
    }
}

fn calculate_multiplier(balance: u64, top_balance: u64) -> f64 {
    1.0 + ((balance as f64) / (top_balance as f64)).min(1.0f64)
}

fn format_duration(seconds: u32) -> String {
    let minutes = seconds / 60;
    let remaining_seconds = seconds % 60;
    format!("{:02}:{:02}", minutes, remaining_seconds)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Mine {
    pub digest: [u8; 16],
    pub nonce: [u8; 8],
}

impl Mine {
    fn to_bytes(&self) -> Vec<u8> {
        let discriminator: [u8; 8] = [170, 66, 140, 123, 15, 20, 224, 194];

        [discriminator.as_slice(), bytemuck::bytes_of(self)].concat()
    }
}

pub fn mine(
    signer: Pubkey,
    pool: Pubkey,
    proof: Pubkey,
    bus: Pubkey,
    solution: Solution
) -> Instruction {
    Instruction {
        program_id: Pubkey::from_str("tPCXnizACpbtbiAn4NQjvbKC9Tp28BBvkjhaSgqDfhH").unwrap(),
        accounts: vec![
            AccountMeta::new(signer, true),
            AccountMeta::new(pool, false),
            AccountMeta::new(proof, false),
            AccountMeta::new(bus, false),
            AccountMeta::new_readonly(CONFIG_ADDRESS, false),
            AccountMeta::new_readonly(
                Pubkey::from_str("oreV2ZymfyeXgNgBdqMkumTqqAprVqgBWQfoYkrtKWQ").unwrap(),
                false
            ),
            AccountMeta::new_readonly(sysvar::slot_hashes::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false)
        ],
        data: (Mine {
            digest: solution.d,
            nonce: solution.n,
        }).to_bytes(),
    }
}
