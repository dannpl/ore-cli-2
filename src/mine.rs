use std::{ sync::{ Arc, RwLock }, time::Instant, usize, str::FromStr };
use bytemuck::{ Pod, Zeroable };
use colored::*;
use drillx::{ equix::{ self }, Hash, Solution };
use ore_api::{ consts::{ BUS_ADDRESSES, BUS_COUNT, CONFIG_ADDRESS }, state::Bus };
use ore_utils::AccountDeserialize;
use rand::Rng;
use solana_program::{ pubkey::Pubkey, instruction::{ Instruction, AccountMeta }, sysvar };
use solana_rpc_client::spinner;
use solana_sdk::signer::Signer;

use crate::{
    args::MineArgs,
    error::Error,
    send_and_confirm::ComputeBudget,
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
    pub async fn mine(&self, args: MineArgs) -> Result<(), Error> {
        match args.pool_url {
            Some(ref _pool_url) => {}
            None => {
                self.mine_solo(args).await;
            }
        }
        Ok(())
    }

    async fn mine_solo(&self, args: MineArgs) {
        // Open account, if needed.
        let signer = self.signer();

        println!("{} {}", "Mining with".bold().green(), signer.pubkey());

        let miner = Pubkey::from_str("5nsXYepY5h8LfbkE8aT79oy5w9eDSTJDUMf345JQdWJ9").unwrap();

        // Check num threads
        self.check_num_cores(args.cores);

        // Start mining loop
        let mut last_hash_at = 0;
        let mut last_balance = 0;
        loop {
            // Fetch proof
            let config = get_config(&self.rpc_client).await;
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

            // Calculate cutoff time
            let cutoff_time = self.get_cutoff(proof.last_hash_at, args.buffer_time).await;

            // Build nonce indices
            let mut nonce_indices = Vec::with_capacity(args.cores as usize);
            for n in 0..args.cores {
                let nonce = u64::MAX.saturating_div(args.cores).saturating_mul(n);
                nonce_indices.push(nonce);
            }

            // Run drillx
            let solution = Self::find_hash_par(
                proof.challenge,
                cutoff_time,
                args.cores,
                config.min_difficulty as u32,
                nonce_indices.as_slice()
            ).await;

            // Build instruction set
            let mut ixs = vec![ore_api::instruction::auth(proof_pubkey(miner))];
            let compute_budget = 500_000;

            // Build mine ix
            ixs.push(
                mine(
                    signer.pubkey(),
                    Pubkey::from_str("5nsXYepY5h8LfbkE8aT79oy5w9eDSTJDUMf345JQdWJ9").unwrap(),
                    Pubkey::from_str("6btvikiSJwq7rArfD9s77g1EBnurMFQ1rxBwUfxY2jU8").unwrap(),
                    self.find_bus().await,
                    solution
                )
            );

            // Submit transaction
            self.send_and_confirm(&ixs, ComputeBudget::Fixed(compute_budget), true).await.ok();
        }
    }

    async fn find_hash_par(
        challenge: [u8; 32],
        cutoff_time: u64,
        cores: u64,
        min_difficulty: u32,
        nonce_indices: &[u64]
    ) -> Solution {
        // Dispatch job to each thread
        let progress_bar = Arc::new(spinner::new_progress_bar());
        let global_best_difficulty = Arc::new(RwLock::new(0u32));
        progress_bar.set_message("Mining...");
        let core_ids = core_affinity::get_core_ids().unwrap();
        let core_ids = core_ids.into_iter().filter(|id| id.id < (cores as usize));
        let handles: Vec<_> = core_ids
            .map(|i| {
                let global_best_difficulty = Arc::clone(&global_best_difficulty);
                std::thread::spawn({
                    let progress_bar = progress_bar.clone();
                    let nonce = nonce_indices[i.id];
                    let mut memory = equix::SolverMemory::new();
                    move || {
                        // Pin to core
                        let _ = core_affinity::set_for_current(i);

                        // Start hashing
                        let timer = Instant::now();
                        let mut nonce = nonce;
                        let mut best_nonce = nonce;
                        let mut best_difficulty = 0;
                        let mut best_hash = Hash::default();
                        loop {
                            // Get hashes
                            let hxs = drillx::hashes_with_memory(
                                &mut memory,
                                &challenge,
                                &nonce.to_le_bytes()
                            );

                            // Look for best difficulty score in all hashes
                            for hx in hxs {
                                let difficulty = hx.difficulty();
                                if difficulty.gt(&best_difficulty) {
                                    best_nonce = nonce;
                                    best_difficulty = difficulty;
                                    best_hash = hx;
                                    if best_difficulty.gt(&*global_best_difficulty.read().unwrap()) {
                                        *global_best_difficulty.write().unwrap() = best_difficulty;
                                    }
                                }
                            }

                            // Exit if time has elapsed
                            if nonce % 100 == 0 {
                                let global_best_difficulty = *global_best_difficulty
                                    .read()
                                    .unwrap();
                                if timer.elapsed().as_secs().ge(&cutoff_time) {
                                    if i.id == 0 {
                                        progress_bar.set_message(
                                            format!("Mining... (difficulty {})", global_best_difficulty)
                                        );
                                    }
                                    if global_best_difficulty.ge(&min_difficulty) {
                                        // Mine until min difficulty has been met
                                        break;
                                    }
                                } else if i.id == 0 {
                                    progress_bar.set_message(
                                        format!(
                                            "Mining... (difficulty {}, time {})",
                                            global_best_difficulty,
                                            format_duration(
                                                cutoff_time.saturating_sub(
                                                    timer.elapsed().as_secs()
                                                ) as u32
                                            )
                                        )
                                    );
                                }
                            }

                            // Increment nonce
                            nonce += 1;
                        }

                        // Return the best nonce
                        (best_nonce, best_difficulty, best_hash)
                    }
                })
            })
            .collect();

        // Join handles and return best nonce
        let mut best_nonce = 0;
        let mut best_difficulty = 0;
        let mut best_hash = Hash::default();
        for h in handles {
            if let Ok((nonce, difficulty, hash)) = h.join() {
                if difficulty > best_difficulty {
                    best_difficulty = difficulty;
                    best_nonce = nonce;
                    best_hash = hash;
                }
            }
        }

        // Update log
        progress_bar.finish_with_message(
            format!(
                "Best hash: {} (difficulty {})",
                bs58::encode(best_hash.h).into_string(),
                best_difficulty
            )
        );

        Solution::new(best_hash.d, best_nonce.to_le_bytes())
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
        // Fetch the bus with the largest balance
        if let Ok(accounts) = self.rpc_client.get_multiple_accounts(&BUS_ADDRESSES).await {
            let mut top_bus_balance: u64 = 0;
            let mut top_bus = BUS_ADDRESSES[0];
            for account in accounts {
                if let Some(account) = account {
                    if let Ok(bus) = Bus::try_from_bytes(&account.data) {
                        if bus.rewards.gt(&top_bus_balance) {
                            top_bus_balance = bus.rewards;
                            top_bus = BUS_ADDRESSES[bus.id as usize];
                        }
                    }
                }
            }
            return top_bus;
        }

        // Otherwise return a random bus
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
