#[macro_use]
extern crate exonum;
#[macro_use]
extern crate failure;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
#[macro_use]
extern crate lazy_static;

pub mod schema {
    use exonum::{
        crypto::{PublicKey, Hash},
        storage::{Fork, MapIndex, Snapshot},
    };

    encoding_struct! {
        struct Bid {
            /// Public key of the bidder
            pub_key: &PublicKey,
            value: u64,
        }
    }

    encoding_struct! {
        struct Lot {
            /// Public key of the lot's owner
            pub_key: &PublicKey,
            descr: &str,
            start_price: u64,
            bids: Vec<Bid>,
        }
    }

    impl Lot {
        pub fn bid(self, bid: Bid) -> Self {
            let mut bids = self.bids();
            bids.push(bid);
            Self::new(self.pub_key(), self.descr(), self.start_price(), bids)
        }
    }

    pub struct AuctionSchema<T> {
        view: T,
    }

    impl<T> AuctionSchema<T> where T: AsRef<Snapshot> {
        pub fn new(view: T) -> Self {
            Self { view }
        }

        pub fn lots(&self) -> MapIndex<&dyn Snapshot, Hash, Lot> {
            MapIndex::new("auction.lots", self.view.as_ref())
        }

        pub fn lot(&self, lot_id: &Hash) -> Option<Lot> {
            self.lots().get(lot_id)
        }
    }

    impl<'a> AuctionSchema<&'a mut Fork> {
        pub fn lots_mut(&mut self) -> MapIndex<&mut Fork, Hash, Lot> {
            MapIndex::new("auction.lots", &mut self.view)
        }
    }
}

pub mod transactions {
    use exonum::crypto::{PublicKey, Hash};

    use service::SERVICE_ID;
    transactions! {
        pub AuctionTransactions {
            const SERVICE_ID = SERVICE_ID;

            struct TxCreateLot {
                /// Public key of the lot's owner.
                pub_key: &PublicKey,
                descr: &str,
                start_price: u64,
            }

            struct TxMakeBid {
                pub_key: &PublicKey,
                lot_id: &Hash,
                value: u64,
            }
        }
    }
}

pub mod errors {
    #![allow(bare_trait_objects)]

    use exonum::blockchain::ExecutionError;
    #[derive(Debug, Fail)]
    #[repr(u8)]
    pub enum Error {
        #[fail(display = "Lot already exists")]
        LotAlreadyExists = 0,
        #[fail(display = "Lot doesn't exist")]
        LotNotExists = 1,
        #[fail(display = "Bid is too small")]
        InsufficientRate = 2,
    }

    impl From<Error> for ExecutionError {
        fn from(value: Error) -> ExecutionError {
            let description = format!("{}", value);
            ExecutionError::with_description(value as u8, description)
        }
    }
}

pub mod contracts {
    use exonum::{
        blockchain::{ExecutionResult, Transaction},
        messages::Message,
        crypto::{CryptoHash},
        storage::Fork,
    };


    use errors::Error;
    use schema::{AuctionSchema, Bid, Lot};
    use transactions::{TxCreateLot, TxMakeBid};

    impl Transaction for TxCreateLot {
        fn verify(&self) -> bool {
//            true
            self.verify_signature(self.pub_key())
        }

        fn execute(&self, view: &mut Fork) -> ExecutionResult {
            let mut schema = AuctionSchema::new(view);
            let lot_id = self.hash();
            if let Some(_) = schema.lot(&lot_id) {
                return Err(Error::LotAlreadyExists)?;
            }

            schema.lots_mut().put(&lot_id,Lot::new(self.pub_key(),
                                                             self.descr(),
                                                             self.start_price(),
                                                             Vec::new()));
            Ok(())
        }
    }

    impl Transaction for TxMakeBid {
        fn verify(&self) -> bool {
//            true
            self.verify_signature(self.pub_key())
        }

        fn execute(&self, view: &mut Fork) -> ExecutionResult {
            let mut schema = AuctionSchema::new(view);
            if let Some(lot) = schema.lot(self.lot_id()) {
                let last_bid: u64 = match lot.bids().last() {
                    Some(val) => val.value(),
                    None => 0
                };

                if last_bid >= self.value() {
                    return Err(Error::InsufficientRate)?;
                }

                schema.lots_mut().put(&self.lot_id()    , lot.bid(Bid::new(self.pub_key(), self.value())));
                Ok(())
            } else {
                Err(Error::LotNotExists)?
            }
        }
    }
}

pub mod api {
    use std::sync::mpsc::{channel, Sender, Receiver};

    use exonum::{
        api::{self, ServiceApiBuilder, ServiceApiState}, blockchain::Transaction,
        crypto::{Hash}, node::TransactionSend,
        node::ApiSender,
//        api::Error
    };

    use schema::{AuctionSchema, Lot};
    use transactions::AuctionTransactions;
    use service::PendingHolder;
    use service::pending_transacations;
    use failure;

    #[derive(Debug, Clone)]
    pub struct AuctionApi {
        pending: PendingHolder
    }

    #[derive(Debug, Serialize, Deserialize, Clone, Copy)]
    pub struct LotQuery {
        pub lot_id: Hash,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct TransactionResponse {
        /// Hash of the transaction.
        pub tx_hash: Hash,
        pub block_num: u64
    }

    impl AuctionApi {
        pub fn get_lot(state: &ServiceApiState, query: LotQuery) -> api::Result<Lot> {
            let snapshot = state.snapshot();
            AuctionSchema::new(snapshot)
                .lot(&query.lot_id)
                .ok_or_else(|| api::Error::NotFound("\"Lot not found\"".to_owned()))
        }

        fn send(transaction: Box<dyn Transaction>, api_sender: &ApiSender) -> Result<(Receiver<u64>, Hash), failure::Error> {
            let tx_hash = transaction.hash();
            let mut pending = pending_transacations.lock().unwrap();
            if pending.contains_key(&tx_hash) {
                bail!("This transaction has already been processed.")
            }

            let (sender, receiver): (Sender<u64>, Receiver<u64>) = channel();
            pending.insert(tx_hash.clone(), Some(sender));
            match api_sender.send(transaction) {
                Err(e) => {
                    pending.remove(&tx_hash);
                    Err(e)
                },
                Ok(()) => {
                    Ok((receiver, tx_hash))
                }
            }
        }

        fn remove_from_pending(tx_hash: &Hash) {
            let mut pending = pending_transacations.lock().unwrap();
            debug_assert!(pending.contains_key(&tx_hash));
            pending.insert(tx_hash.clone(), None);
        }

        pub fn post_transaction(
            state: &ServiceApiState,
            query: AuctionTransactions,
        ) -> api::Result<TransactionResponse> {
            match AuctionApi::send(query.into(), state.sender()) {
                Ok((receiver, tx_hash)) => {
                    let block_num = receiver.recv().unwrap();
                    AuctionApi::remove_from_pending(&tx_hash);
                    Ok(TransactionResponse { tx_hash, block_num })
                },
                Err(e) => Err(api::Error::InternalError(e))
            }
        }

        pub fn wire(builder: &mut ServiceApiBuilder) {
            builder
                .public_scope()
                .endpoint("lot", Self::get_lot)
                .endpoint_mut("create", Self::post_transaction)
                .endpoint_mut("bid", Self::post_transaction);
        }
    }
}

pub mod service {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, mpsc::Sender};

    use exonum::{
        api::ServiceApiBuilder,
        blockchain::{Service, Transaction, TransactionSet, ServiceContext, Schema},
        crypto::Hash,
        encoding, messages::RawTransaction, storage::Snapshot,
    };

    use api::AuctionApi;
    use transactions::AuctionTransactions;

    pub const SERVICE_ID: u16 = 1;
    pub type PendingHolder = Arc<Mutex<HashMap<Hash, Option<Sender<u64>>>>>;
    lazy_static! {
        pub static ref pending_transacations: PendingHolder = Arc::new(Mutex::new(HashMap::new()));
    }

    pub struct AuctionService;

    impl Service for AuctionService {
        fn service_id(&self) -> u16 {
            SERVICE_ID
        }

        fn service_name(&self) -> &'static str {
            "auction"
        }

        fn state_hash(&self, _: &dyn Snapshot) -> Vec<Hash> {
            vec![]
        }

        fn tx_from_raw(
            &self,
            raw: RawTransaction,
        ) -> Result<Box<dyn Transaction>, encoding::Error> {
            let tx = AuctionTransactions::tx_from_raw(raw)?;
            Ok(tx.into())
        }

        fn after_commit(&self, context: &ServiceContext) {
            let height = context.height();
            let schema = Schema::new(context.snapshot());
            let block = schema.block_transactions(height);
            let pending = &*pending_transacations.lock().unwrap();
            for tx_hash in block.iter() {
                if let Some(Some(sender)) = pending.get(&tx_hash) {
                    sender.send(height.0).unwrap();
                }
            }
        }

        fn wire_api(&self, builder: &mut ServiceApiBuilder) {
            AuctionApi::wire(builder);
        }
    }

    impl AuctionService {
        pub fn new() -> Self {
            Self{}
        }
    }
}
