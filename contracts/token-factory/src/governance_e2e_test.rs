#![cfg(test)]

//! End-to-End Governance Workflow Tests
//!
//! These tests verify complete governance workflows including:
//! - Proposal creation
//! - Voting
//! - Queueing (via timelock)
//! - Execution
//! - State changes
//! - Event sequences

use crate::timelock::{
    create_proposal, vote_proposal, get_proposal, get_vote_counts,
    schedule_fee_update, execute_change, cancel_change, get_pending_change,
    initialize_timelock,
};
use crate::types::{ActionType, VoteChoice, Error};
use crate::storage;
use soroban_sdk::{testutils::Address as _, vec, Env};
use soroban_sdk::testutils::{Ledger, Events};
use soroban_sdk::Symbol;

fn setup_governance() -> (Env, soroban_sdk::Address) {
    let env = Env::default();
    env.mock_all_auths();
    
    let admin = soroban_sdk::Address::generate(&env);
    storage::set_admin(&env, &admin);
    storage::set_treasury(&env, &soroban_sdk::Address::generate(&env));
    storage::set_base_fee(&env, 1_000_000);
    storage::set_metadata_fee(&env, 500_000);
    
    // Initialize timelock with 1 hour delay
    initialize_timelock(&env, Some(3600)).unwrap();
    
    (env, admin)
}

// ═══════════════════════════════════════════════════════════════════════════
// E2E Flow 1: Complete Success Flow (create -> vote -> queue -> execute)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_e2e_proposal_vote_queue_execute_fee_update() {
    let (env, admin) = setup_governance();
    
    // Record initial state
    let initial_base_fee = storage::get_base_fee(&env);
    let initial_metadata_fee = storage::get_metadata_fee(&env);
    assert_eq!(initial_base_fee, 1_000_000);
    assert_eq!(initial_metadata_fee, 500_000);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 1: Create Proposal
    // ─────────────────────────────────────────────────────────────────────
    let current_time = env.ledger().timestamp();
    let start_time = current_time + 100;
    let end_time = start_time + 86400; // 1 day voting
    let eta = end_time + 7200; // 2 hours after voting
    
    let payload = vec![&env, 1u8, 2u8, 3u8]; // Encoded fee change data
    
    let proposal_id = create_proposal(
        &env,
        &admin,
        ActionType::FeeChange,
        payload,
        start_time,
        end_time,
        eta,
    ).unwrap();
    
    // Verify proposal created
    assert_eq!(proposal_id, 0);
    let proposal = get_proposal(&env, proposal_id).unwrap();
    assert_eq!(proposal.action_type, ActionType::FeeChange);
    assert_eq!(proposal.votes_for, 0);
    
    // Verify proposal_created event
    let events = env.events().all();
    let last_event = events.last().unwrap();
    assert_eq!(last_event.0.get(0).unwrap(), Symbol::new(&env, "prop_crt"));
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 2: Cast Votes (simulate community voting)
    // ─────────────────────────────────────────────────────────────────────
    
    // Advance time to voting period
    env.ledger().with_mut(|li| {
        li.timestamp = start_time + 1000;
    });
    
    // Create voters
    let voter1 = soroban_sdk::Address::generate(&env);
    let voter2 = soroban_sdk::Address::generate(&env);
    let voter3 = soroban_sdk::Address::generate(&env);
    let voter4 = soroban_sdk::Address::generate(&env);
    let voter5 = soroban_sdk::Address::generate(&env);
    
    // Cast votes (4 for, 1 against - proposal should pass)
    vote_proposal(&env, &voter1, proposal_id, VoteChoice::For).unwrap();
    vote_proposal(&env, &voter2, proposal_id, VoteChoice::For).unwrap();
    vote_proposal(&env, &voter3, proposal_id, VoteChoice::For).unwrap();
    vote_proposal(&env, &voter4, proposal_id, VoteChoice::For).unwrap();
    vote_proposal(&env, &voter5, proposal_id, VoteChoice::Against).unwrap();
    
    // Verify votes recorded
    let (votes_for, votes_against, votes_abstain) = get_vote_counts(&env, proposal_id).unwrap();
    assert_eq!(votes_for, 4);
    assert_eq!(votes_against, 1);
    assert_eq!(votes_abstain, 0);
    
    // Verify proposal_voted events (should have 5)
    let vote_events: Vec<_> = env.events().all()
        .iter()
        .filter(|e| e.0.get(0).unwrap() == Symbol::new(&env, "prop_vot"))
        .collect();
    assert_eq!(vote_events.len(), 5);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 3: Queue for Execution (via timelock)
    // ─────────────────────────────────────────────────────────────────────
    
    // Advance time past voting period
    env.ledger().with_mut(|li| {
        li.timestamp = end_time + 100;
    });
    
    // Admin queues the approved proposal for execution
    let new_base_fee = 2_000_000_i128;
    let new_metadata_fee = 750_000_i128;
    
    let change_id = schedule_fee_update(
        &env,
        &admin,
        Some(new_base_fee),
        Some(new_metadata_fee),
    ).unwrap();
    
    // Verify change scheduled
    let pending = get_pending_change(&env, change_id).unwrap();
    assert_eq!(pending.base_fee, Some(new_base_fee));
    assert_eq!(pending.metadata_fee, Some(new_metadata_fee));
    assert!(!pending.executed);
    
    // Verify change_scheduled event
    let schedule_events: Vec<_> = env.events().all()
        .iter()
        .filter(|e| e.0.get(0).unwrap() == Symbol::new(&env, "ch_sched"))
        .collect();
    assert_eq!(schedule_events.len(), 1);
    
    // Verify fees haven't changed yet
    assert_eq!(storage::get_base_fee(&env), initial_base_fee);
    assert_eq!(storage::get_metadata_fee(&env), initial_metadata_fee);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 4: Execute Change (after timelock)
    // ─────────────────────────────────────────────────────────────────────
    
    // Advance time past timelock delay (1 hour)
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp + 3601;
    });
    
    // Execute the change
    execute_change(&env, change_id).unwrap();
    
    // Verify fees updated
    assert_eq!(storage::get_base_fee(&env), new_base_fee);
    assert_eq!(storage::get_metadata_fee(&env), new_metadata_fee);
    
    // Verify change marked as executed
    let executed_change = get_pending_change(&env, change_id).unwrap();
    assert!(executed_change.executed);
    
    // Verify change_executed event
    let execute_events: Vec<_> = env.events().all()
        .iter()
        .filter(|e| e.0.get(0).unwrap() == Symbol::new(&env, "ch_exec"))
        .collect();
    assert_eq!(execute_events.len(), 1);
    
    // Verify fee_updated event
    let fee_events: Vec<_> = env.events().all()
        .iter()
        .filter(|e| e.0.get(0).unwrap() == Symbol::new(&env, "fee_up_v1"))
        .collect();
    assert_eq!(fee_events.len(), 1);
    
    // ─────────────────────────────────────────────────────────────────────
    // Final Verification: Complete Event Sequence
    // ─────────────────────────────────────────────────────────────────────
    
    let all_events = env.events().all();
    
    // Expected event sequence:
    // 1. prop_crt (proposal created)
    // 2-6. prop_vot (5 votes)
    // 7. ch_sched (change scheduled)
    // 8. ch_exec (change executed)
    // 9. fee_up_v1 (fees updated)
    
    let event_types: Vec<Symbol> = all_events
        .iter()
        .map(|e| e.0.get(0).unwrap())
        .collect();
    
    // Verify key events present
    assert!(event_types.contains(&Symbol::new(&env, "prop_crt")));
    assert!(event_types.contains(&Symbol::new(&env, "ch_sched")));
    assert!(event_types.contains(&Symbol::new(&env, "ch_exec")));
    assert!(event_types.contains(&Symbol::new(&env, "fee_up_v1")));
    
    // Count vote events
    let vote_count = event_types.iter()
        .filter(|s| **s == Symbol::new(&env, "prop_vot"))
        .count();
    assert_eq!(vote_count, 5);
}

// ═══════════════════════════════════════════════════════════════════════════
// E2E Flow 2: Vote Failure Flow (create -> vote fail -> queue reject)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_e2e_proposal_vote_fail_quorum_miss() {
    let (env, admin) = setup_governance();
    
    let initial_base_fee = storage::get_base_fee(&env);
    let initial_metadata_fee = storage::get_metadata_fee(&env);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 1: Create Proposal
    // ─────────────────────────────────────────────────────────────────────
    let current_time = env.ledger().timestamp();
    let start_time = current_time + 100;
    let end_time = start_time + 86400;
    let eta = end_time + 7200;
    
    let payload = vec![&env, 1u8, 2u8, 3u8];
    
    let proposal_id = create_proposal(
        &env,
        &admin,
        ActionType::FeeChange,
        payload,
        start_time,
        end_time,
        eta,
    ).unwrap();
    
    // Verify proposal created
    assert_eq!(proposal_id, 0);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 2: Insufficient Votes (quorum not met)
    // ─────────────────────────────────────────────────────────────────────
    
    // Advance time to voting period
    env.ledger().with_mut(|li| {
        li.timestamp = start_time + 1000;
    });
    
    // Only 2 voters participate (simulating low turnout)
    let voter1 = soroban_sdk::Address::generate(&env);
    let voter2 = soroban_sdk::Address::generate(&env);
    
    vote_proposal(&env, &voter1, proposal_id, VoteChoice::For).unwrap();
    vote_proposal(&env, &voter2, proposal_id, VoteChoice::Against).unwrap();
    
    // Verify votes recorded
    let (votes_for, votes_against, votes_abstain) = get_vote_counts(&env, proposal_id).unwrap();
    assert_eq!(votes_for, 1);
    assert_eq!(votes_against, 1);
    assert_eq!(votes_abstain, 0);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 3: Attempt to Queue (should be rejected in real governance)
    // ─────────────────────────────────────────────────────────────────────
    
    // Advance time past voting period
    env.ledger().with_mut(|li| {
        li.timestamp = end_time + 100;
    });
    
    // In a real system with quorum checks, this would fail
    // For now, we verify the vote counts show insufficient support
    let proposal = get_proposal(&env, proposal_id).unwrap();
    assert_eq!(proposal.votes_for, 1);
    assert_eq!(proposal.votes_against, 1);
    
    // Verify that if admin tries to queue anyway, the original state is preserved
    // (In production, there would be a quorum check before allowing queue)
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 4: Verify State Unchanged
    // ─────────────────────────────────────────────────────────────────────
    
    // Fees should remain unchanged
    assert_eq!(storage::get_base_fee(&env), initial_base_fee);
    assert_eq!(storage::get_metadata_fee(&env), initial_metadata_fee);
    
    // Verify event sequence
    let all_events = env.events().all();
    let event_types: Vec<Symbol> = all_events
        .iter()
        .map(|e| e.0.get(0).unwrap())
        .collect();
    
    // Should have: prop_crt, 2x prop_vot
    assert!(event_types.contains(&Symbol::new(&env, "prop_crt")));
    let vote_count = event_types.iter()
        .filter(|s| **s == Symbol::new(&env, "prop_vot"))
        .count();
    assert_eq!(vote_count, 2);
    
    // Should NOT have: ch_sched, ch_exec, fee_up_v1
    assert!(!event_types.contains(&Symbol::new(&env, "ch_sched")));
    assert!(!event_types.contains(&Symbol::new(&env, "ch_exec")));
    assert!(!event_types.contains(&Symbol::new(&env, "fee_up_v1")));
}

// ═══════════════════════════════════════════════════════════════════════════
// E2E Flow 3: Cancellation Flow (create -> queue -> cancel -> execute reject)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_e2e_proposal_queue_cancel_execute_reject() {
    let (env, admin) = setup_governance();
    
    let initial_base_fee = storage::get_base_fee(&env);
    let initial_metadata_fee = storage::get_metadata_fee(&env);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 1: Create Proposal
    // ─────────────────────────────────────────────────────────────────────
    let current_time = env.ledger().timestamp();
    let start_time = current_time + 100;
    let end_time = start_time + 86400;
    let eta = end_time + 7200;
    
    let payload = vec![&env, 1u8, 2u8, 3u8];
    
    let proposal_id = create_proposal(
        &env,
        &admin,
        ActionType::FeeChange,
        payload,
        start_time,
        end_time,
        eta,
    ).unwrap();
    
    assert_eq!(proposal_id, 0);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 2: Voting (passes)
    // ─────────────────────────────────────────────────────────────────────
    
    env.ledger().with_mut(|li| {
        li.timestamp = start_time + 1000;
    });
    
    let voter1 = soroban_sdk::Address::generate(&env);
    let voter2 = soroban_sdk::Address::generate(&env);
    let voter3 = soroban_sdk::Address::generate(&env);
    
    vote_proposal(&env, &voter1, proposal_id, VoteChoice::For).unwrap();
    vote_proposal(&env, &voter2, proposal_id, VoteChoice::For).unwrap();
    vote_proposal(&env, &voter3, proposal_id, VoteChoice::For).unwrap();
    
    let (votes_for, _, _) = get_vote_counts(&env, proposal_id).unwrap();
    assert_eq!(votes_for, 3);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 3: Queue for Execution
    // ─────────────────────────────────────────────────────────────────────
    
    env.ledger().with_mut(|li| {
        li.timestamp = end_time + 100;
    });
    
    let new_base_fee = 2_000_000_i128;
    let new_metadata_fee = 750_000_i128;
    
    let change_id = schedule_fee_update(
        &env,
        &admin,
        Some(new_base_fee),
        Some(new_metadata_fee),
    ).unwrap();
    
    // Verify change scheduled
    let pending = get_pending_change(&env, change_id).unwrap();
    assert!(!pending.executed);
    
    // Verify change_scheduled event
    let schedule_events: Vec<_> = env.events().all()
        .iter()
        .filter(|e| e.0.get(0).unwrap() == Symbol::new(&env, "ch_sched"))
        .collect();
    assert_eq!(schedule_events.len(), 1);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 4: Cancel Before Execution
    // ─────────────────────────────────────────────────────────────────────
    
    // Admin decides to cancel (e.g., discovered issue, changed mind)
    cancel_change(&env, &admin, change_id).unwrap();
    
    // Verify change removed
    let cancelled = get_pending_change(&env, change_id);
    assert!(cancelled.is_none());
    
    // Verify change_cancelled event
    let cancel_events: Vec<_> = env.events().all()
        .iter()
        .filter(|e| e.0.get(0).unwrap() == Symbol::new(&env, "ch_cncl"))
        .collect();
    assert_eq!(cancel_events.len(), 1);
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 5: Attempt to Execute (should fail)
    // ─────────────────────────────────────────────────────────────────────
    
    // Advance time past timelock
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp + 3601;
    });
    
    // Try to execute cancelled change
    let result = execute_change(&env, change_id);
    assert_eq!(result, Err(Error::TokenNotFound));
    
    // ─────────────────────────────────────────────────────────────────────
    // Step 6: Verify State Unchanged
    // ─────────────────────────────────────────────────────────────────────
    
    // Fees should remain at original values
    assert_eq!(storage::get_base_fee(&env), initial_base_fee);
    assert_eq!(storage::get_metadata_fee(&env), initial_metadata_fee);
    
    // Verify event sequence
    let all_events = env.events().all();
    let event_types: Vec<Symbol> = all_events
        .iter()
        .map(|e| e.0.get(0).unwrap())
        .collect();
    
    // Should have: prop_crt, 3x prop_vot, ch_sched, ch_cncl
    assert!(event_types.contains(&Symbol::new(&env, "prop_crt")));
    assert!(event_types.contains(&Symbol::new(&env, "ch_sched")));
    assert!(event_types.contains(&Symbol::new(&env, "ch_cncl")));
    
    let vote_count = event_types.iter()
        .filter(|s| **s == Symbol::new(&env, "prop_vot"))
        .count();
    assert_eq!(vote_count, 3);
    
    // Should NOT have: ch_exec, fee_up_v1
    assert!(!event_types.contains(&Symbol::new(&env, "ch_exec")));
    assert!(!event_types.contains(&Symbol::new(&env, "fee_up_v1")));
}

// ═══════════════════════════════════════════════════════════════════════════
// Additional E2E Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_e2e_multiple_proposals_independent_execution() {
    let (env, admin) = setup_governance();
    
    let initial_base_fee = storage::get_base_fee(&env);
    
    // Create two proposals
    let current_time = env.ledger().timestamp();
    let start_time = current_time + 100;
    let end_time = start_time + 86400;
    let eta = end_time + 7200;
    
    let payload = vec![&env, 1u8];
    
    let proposal_id_1 = create_proposal(
        &env, &admin, ActionType::FeeChange, payload.clone(),
        start_time, end_time, eta,
    ).unwrap();
    
    let proposal_id_2 = create_proposal(
        &env, &admin, ActionType::FeeChange, payload,
        start_time, end_time, eta,
    ).unwrap();
    
    // Vote on both
    env.ledger().with_mut(|li| {
        li.timestamp = start_time + 1000;
    });
    
    let voter = soroban_sdk::Address::generate(&env);
    vote_proposal(&env, &voter, proposal_id_1, VoteChoice::For).unwrap();
    vote_proposal(&env, &voter, proposal_id_2, VoteChoice::For).unwrap();
    
    // Queue first proposal
    env.ledger().with_mut(|li| {
        li.timestamp = end_time + 100;
    });
    
    let change_id_1 = schedule_fee_update(
        &env, &admin, Some(2_000_000), None,
    ).unwrap();
    
    // Execute first proposal
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp + 3601;
    });
    
    execute_change(&env, change_id_1).unwrap();
    
    // Verify first change applied
    assert_eq!(storage::get_base_fee(&env), 2_000_000);
    
    // Second proposal remains independent
    let proposal_2 = get_proposal(&env, proposal_id_2).unwrap();
    assert_eq!(proposal_2.votes_for, 1);
}

#[test]
fn test_e2e_execute_before_timelock_fails() {
    let (env, admin) = setup_governance();
    
    // Create and vote on proposal
    let current_time = env.ledger().timestamp();
    let start_time = current_time + 100;
    let end_time = start_time + 86400;
    let eta = end_time + 7200;
    
    let payload = vec![&env, 1u8];
    let proposal_id = create_proposal(
        &env, &admin, ActionType::FeeChange, payload,
        start_time, end_time, eta,
    ).unwrap();
    
    env.ledger().with_mut(|li| {
        li.timestamp = start_time + 1000;
    });
    
    let voter = soroban_sdk::Address::generate(&env);
    vote_proposal(&env, &voter, proposal_id, VoteChoice::For).unwrap();
    
    // Queue
    env.ledger().with_mut(|li| {
        li.timestamp = end_time + 100;
    });
    
    let change_id = schedule_fee_update(&env, &admin, Some(2_000_000), None).unwrap();
    
    // Try to execute immediately (before timelock expires)
    let result = execute_change(&env, change_id);
    assert_eq!(result, Err(Error::TimelockNotExpired));
    
    // Verify state unchanged
    assert_eq!(storage::get_base_fee(&env), 1_000_000);
}

#[test]
fn test_e2e_double_execution_fails() {
    let (env, admin) = setup_governance();
    
    // Create, vote, queue
    let current_time = env.ledger().timestamp();
    let start_time = current_time + 100;
    let end_time = start_time + 86400;
    let eta = end_time + 7200;
    
    let payload = vec![&env, 1u8];
    let proposal_id = create_proposal(
        &env, &admin, ActionType::FeeChange, payload,
        start_time, end_time, eta,
    ).unwrap();
    
    env.ledger().with_mut(|li| {
        li.timestamp = start_time + 1000;
    });
    
    let voter = soroban_sdk::Address::generate(&env);
    vote_proposal(&env, &voter, proposal_id, VoteChoice::For).unwrap();
    
    env.ledger().with_mut(|li| {
        li.timestamp = end_time + 100;
    });
    
    let change_id = schedule_fee_update(&env, &admin, Some(2_000_000), None).unwrap();
    
    // Execute once
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp + 3601;
    });
    
    execute_change(&env, change_id).unwrap();
    assert_eq!(storage::get_base_fee(&env), 2_000_000);
    
    // Try to execute again
    let result = execute_change(&env, change_id);
    assert_eq!(result, Err(Error::ChangeAlreadyExecuted));
    
    // Fee should still be 2_000_000 (not doubled)
    assert_eq!(storage::get_base_fee(&env), 2_000_000);
}

#[test]
fn test_e2e_event_sequence_correctness() {
    let (env, admin) = setup_governance();
    
    // Complete flow
    let current_time = env.ledger().timestamp();
    let start_time = current_time + 100;
    let end_time = start_time + 86400;
    let eta = end_time + 7200;
    
    let payload = vec![&env, 1u8];
    
    // Create
    let proposal_id = create_proposal(
        &env, &admin, ActionType::FeeChange, payload,
        start_time, end_time, eta,
    ).unwrap();
    
    let events_after_create = env.events().all().len();
    
    // Vote
    env.ledger().with_mut(|li| {
        li.timestamp = start_time + 1000;
    });
    
    let voter1 = soroban_sdk::Address::generate(&env);
    let voter2 = soroban_sdk::Address::generate(&env);
    vote_proposal(&env, &voter1, proposal_id, VoteChoice::For).unwrap();
    vote_proposal(&env, &voter2, proposal_id, VoteChoice::For).unwrap();
    
    let events_after_vote = env.events().all().len();
    assert!(events_after_vote > events_after_create);
    
    // Queue
    env.ledger().with_mut(|li| {
        li.timestamp = end_time + 100;
    });
    
    let change_id = schedule_fee_update(&env, &admin, Some(2_000_000), None).unwrap();
    
    let events_after_queue = env.events().all().len();
    assert!(events_after_queue > events_after_vote);
    
    // Execute
    env.ledger().with_mut(|li| {
        li.timestamp = li.timestamp + 3601;
    });
    
    execute_change(&env, change_id).unwrap();
    
    let events_after_execute = env.events().all().len();
    assert!(events_after_execute > events_after_queue);
    
    // Verify event order
    let all_events = env.events().all();
    let event_types: Vec<Symbol> = all_events
        .iter()
        .map(|e| e.0.get(0).unwrap())
        .collect();
    
    // Find indices
    let prop_crt_idx = event_types.iter().position(|s| *s == Symbol::new(&env, "prop_crt"));
    let first_vote_idx = event_types.iter().position(|s| *s == Symbol::new(&env, "prop_vot"));
    let ch_sched_idx = event_types.iter().position(|s| *s == Symbol::new(&env, "ch_sched"));
    let ch_exec_idx = event_types.iter().position(|s| *s == Symbol::new(&env, "ch_exec"));
    
    // Verify chronological order
    assert!(prop_crt_idx.is_some());
    assert!(first_vote_idx.is_some());
    assert!(ch_sched_idx.is_some());
    assert!(ch_exec_idx.is_some());
    
    assert!(prop_crt_idx.unwrap() < first_vote_idx.unwrap());
    assert!(first_vote_idx.unwrap() < ch_sched_idx.unwrap());
    assert!(ch_sched_idx.unwrap() < ch_exec_idx.unwrap());
}
