// Built-in deps
use std::str::FromStr;
// External imports
use bigdecimal::BigDecimal;
use web3::types::{H256, U256};
// Workspace imports
use models::{
    ethereum::{ETHOperation, OperationType},
    node::{block::Block, BlockNumber, Fr},
    Action, Operation,
};
// Local imports
use crate::tests::db_test;
use crate::{chain::block::BlockSchema, ethereum::EthereumSchema, StorageProcessor};

/// Creates a sample operation to be stored in `operations` table.
/// This function is required since `eth_operations` table is linked to
/// the `operations` table by the operation id.
pub fn get_operation(block_number: BlockNumber) -> Operation {
    Operation {
        id: None,
        action: Action::Commit,
        block: Block {
            block_number,
            new_root_hash: Fr::default(),
            fee_account: 0,
            block_transactions: Vec::new(),
            processed_priority_ops: (0, 0),
        },
        accounts_updated: Default::default(),
    }
}

/// Parameters for `EthereumSchema::save_operation_eth_tx` method.
#[derive(Debug)]
pub struct EthereumTxParams {
    op_type: String,
    op_id: i64,
    hash: H256,
    deadline_block: u64,
    nonce: u32,
    gas_price: BigDecimal,
    raw_tx: Vec<u8>,
}

impl EthereumTxParams {
    pub fn new(op_type: String, op_id: i64, nonce: u32) -> Self {
        Self {
            op_type,
            op_id,
            hash: H256::from_low_u64_ne(op_id as u64),
            deadline_block: 100,
            nonce,
            gas_price: 1000.into(),
            raw_tx: Default::default(),
        }
    }

    pub fn to_eth_op(&self, db_id: i64) -> ETHOperation {
        let op_type = OperationType::from_str(self.op_type.as_ref())
            .expect("Stored operation type must have a valid value");
        let last_used_gas_price = U256::from_str(&self.gas_price.to_string()).unwrap();
        let used_tx_hashes = vec![self.hash.clone()];

        ETHOperation {
            id: db_id,
            op_type,
            nonce: self.nonce.into(),
            last_deadline_block: self.deadline_block,
            last_used_gas_price,
            used_tx_hashes,
            encoded_tx_data: self.raw_tx.clone(),
            confirmed: false,
            final_hash: None,
        }
    }
}

/// Verifies that on a fresh database no bogus operations are loaded.
#[test]
#[cfg_attr(not(feature = "db_test"), ignore)]
fn ethereum_empty_load() {
    let conn = StorageProcessor::establish_connection().unwrap();
    db_test(conn.conn(), || {
        let unconfirmed_operations = EthereumSchema(&conn).load_unconfirmed_operations()?;
        assert!(unconfirmed_operations.is_empty());

        Ok(())
    });
}

/// Checks the basic Ethereum storage workflow:
/// - Store the operations in the block schema.
/// - Save the Ethereum tx.
/// - Check that saved tx can be loaded.
/// - Save another Ethereum tx for the same operation.
/// - Check that both txs can be loaded.
/// - Make the operation as completed.
/// - Check that now txs aren't loaded.
#[test]
#[cfg_attr(not(feature = "db_test"), ignore)]
fn ethereum_storage() {
    let conn = StorageProcessor::establish_connection().unwrap();
    db_test(conn.conn(), || {
        EthereumSchema(&conn).initialize_eth_data()?;

        let unconfirmed_operations = EthereumSchema(&conn).load_unconfirmed_operations()?;
        assert!(unconfirmed_operations.is_empty());

        // Store operation with ID 1.
        let block_number = 1;
        let operation = BlockSchema(&conn).execute_operation(get_operation(block_number))?;

        // Store the Ethereum transaction.
        let params = EthereumTxParams::new("commit".into(), operation.id.unwrap(), 1);
        EthereumSchema(&conn).save_new_eth_tx(
            OperationType::Commit,
            Some(params.op_id),
            params.hash,
            params.deadline_block,
            params.nonce,
            params.gas_price.clone(),
            params.raw_tx.clone(),
        )?;

        // Check that it can be loaded.
        let unconfirmed_operations = EthereumSchema(&conn).load_unconfirmed_operations()?;
        let eth_op = unconfirmed_operations[0].0.clone();
        let op = unconfirmed_operations[0]
            .1
            .clone()
            .expect("No Operation entry");
        assert_eq!(op.id, operation.id);
        // Load the database ID, since we can't predict it for sure.
        assert_eq!(eth_op, params.to_eth_op(eth_op.id));

        // Store operation with ID 2.
        let block_number = 2;
        let operation_2 = BlockSchema(&conn).execute_operation(get_operation(block_number))?;

        // Create one more Ethereum transaction.
        let params_2 = EthereumTxParams::new("commit".into(), operation_2.id.unwrap(), 2);
        EthereumSchema(&conn).save_new_eth_tx(
            OperationType::Commit,
            Some(params_2.op_id),
            params_2.hash,
            params_2.deadline_block,
            params_2.nonce,
            params_2.gas_price.clone(),
            params_2.raw_tx.clone(),
        )?;

        // Check that we now can load two operations.
        let unconfirmed_operations = EthereumSchema(&conn).load_unconfirmed_operations()?;
        assert_eq!(unconfirmed_operations.len(), 2);
        let eth_op = unconfirmed_operations[1].0.clone();
        let op = unconfirmed_operations[1]
            .1
            .clone()
            .expect("No Operation entry");
        assert_eq!(op.id, operation_2.id);
        assert_eq!(eth_op, params_2.to_eth_op(eth_op.id));

        // Make the transaction as completed.
        EthereumSchema(&conn).confirm_eth_tx(&params_2.hash)?;

        // Now there should be only one unconfirmed operation.
        let unconfirmed_operations = EthereumSchema(&conn).load_unconfirmed_operations()?;
        assert_eq!(unconfirmed_operations.len(), 1);

        // Check that stats are updated as well.
        let updated_stats = EthereumSchema(&conn).load_stats()?;

        assert_eq!(updated_stats.commit_ops, 2);
        assert_eq!(updated_stats.verify_ops, 0);
        assert_eq!(updated_stats.withdraw_ops, 0);

        Ok(())
    });
}

/// Check that stored nonce starts with 0 and is incremented after every getting.
#[test]
#[cfg_attr(not(feature = "db_test"), ignore)]
fn eth_nonce() {
    let conn = StorageProcessor::establish_connection().unwrap();
    db_test(conn.conn(), || {
        EthereumSchema(&conn).initialize_eth_data()?;

        for expected_next_nonce in 0..5 {
            let actual_next_nonce = EthereumSchema(&conn).get_next_nonce()?;

            assert_eq!(actual_next_nonce, expected_next_nonce);
        }

        Ok(())
    });
}
