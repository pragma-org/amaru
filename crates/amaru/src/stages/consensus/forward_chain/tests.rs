use super::{
    client_state::find_headers_between,
    test_infra::{
        hash, mk_store, ClientMsg, Setup, BRANCH_47, CHAIN_47, LOST_47, TIP_47, WINNER_47,
    },
};
use crate::stages::{AsTip, PallasPoint};
use amaru_consensus::IsHeader;
use pallas_network::miniprotocols::chainsync::Tip;
use pallas_network::miniprotocols::Point;

#[test]
fn test_mk_store() {
    let store = mk_store(CHAIN_47);
    assert_eq!(store.len(), 48);
    let chain = store.get_chain(TIP_47);
    assert_eq!(chain.len(), 47);
    assert_eq!(chain[0].header_body.slot, 31);
    assert_eq!(chain[46].header_body.slot, 990);
    assert_eq!(chain[6].block_height(), 7);
}

#[test]
fn find_headers_between_tip_and_tip() {
    let store = mk_store(CHAIN_47);

    let tip = store.get_point(TIP_47);
    let points = [store.get_point(TIP_47)];

    let (ops, Tip(p, h)) = find_headers_between(&store, &tip, &points).unwrap();
    assert_eq!((ops, p, h), (vec![], tip, 47));
}

#[test]
fn find_headers_between_tip_and_branch() {
    let store = mk_store(CHAIN_47);

    let tip = store.get_point(TIP_47);
    let points = [store.get_point(BRANCH_47)];
    let peer = store.get_point(BRANCH_47);

    let (ops, Tip(p, h)) = find_headers_between(&store, &tip, &points).unwrap();
    assert_eq!(
        (ops.len() as u64, p, h),
        (
            store.get_height(TIP_47) - store.get_height(BRANCH_47),
            peer,
            store.get_height(BRANCH_47)
        )
    );
}

#[test]
fn find_headers_between_tip_and_branches() {
    let store = mk_store(CHAIN_47);

    let tip = store.get_point(TIP_47);
    // Note that the below scheme does not match the documented behaviour, which shall pick the first from
    // the list that is on the same chain. But that doesn't make sense to me at all.
    let points = [
        store.get_point(BRANCH_47), // this will lose to the (taller) winner
        store.get_point(LOST_47),   // this is not on the same chain
        store.get_point(WINNER_47), // this is the winner after the branch
    ];
    let peer = store.get_point(WINNER_47);

    let (ops, Tip(p, h)) = find_headers_between(&store, &tip, &points).unwrap();
    assert_eq!(
        (ops.len() as u64, p, h),
        (
            store.get_height(TIP_47) - store.get_height(WINNER_47),
            peer,
            store.get_height(WINNER_47)
        )
    );
}

#[test]
fn find_headers_between_tip_and_lost() {
    let store = mk_store(CHAIN_47);

    let tip = store.get_point(TIP_47);
    let points = [store.get_point(LOST_47)];

    let result = find_headers_between(&store, &tip, &points).unwrap();
    assert_eq!(result.0.len() as u64, store.get_height(TIP_47));
    assert_eq!(result.1 .0, Point::Origin);
    assert_eq!(result.1 .1, 0);
}

#[test]
fn test_chain_sync() {
    let mut setup = Setup::new(LOST_47);
    let chain = setup.store.get_chain(TIP_47);
    let lost = setup.store.get(&hash(LOST_47)).unwrap().clone();

    let mut client = setup.connect();

    let (p, t) = client.find_intersect(vec![chain[6].pallas_point()]);

    assert_eq!(p, Some(setup.store.get_point(BRANCH_47)));
    assert_eq!(t.0, lost.pallas_point());
    assert_eq!(t.1, lost.block_height());

    let headers = client.recv_until_await();
    assert_eq!(
        headers,
        vec![ClientMsg::Forward(lost.clone(), lost.as_tip())]
    );

    setup.send_backward(BRANCH_47);
    setup.send_validated(WINNER_47);
    setup.send_validated(&chain[8].hash().to_string());
    let msg = client.recv_after_await();
    assert_eq!(
        msg,
        // out tip comes out as chain[6] here because previously client.recv_until_await already
        // asked for the next op, which means the Backward got sent before the BlockValidated
        // updated the `our_tip` pointer
        ClientMsg::Backward(chain[6].pallas_point(), chain[6].as_tip())
    );

    let headers = client.recv_until_await();
    assert_eq!(
        headers,
        vec![
            ClientMsg::Forward(chain[7].clone(), chain[8].as_tip()),
            ClientMsg::Forward(chain[8].clone(), chain[8].as_tip()),
        ]
    );

    // Note: there’s no way to shut down the gasket stage without logging to ERRORs, sorry
}

#[test]
fn test_sync_optimising_rollback() {
    let mut setup = Setup::new(LOST_47);
    let chain = setup.store.get_chain(TIP_47);
    let lost = setup.store.get(&hash(LOST_47)).unwrap().clone();

    let mut client = setup.connect();
    client.find_intersect(vec![]).0.expect("no intersection");

    let msgs = client.recv_n::<4>();
    assert_eq!(
        msgs,
        [
            ClientMsg::Forward(chain[0].clone(), lost.as_tip()),
            ClientMsg::Forward(chain[1].clone(), lost.as_tip()),
            ClientMsg::Forward(chain[2].clone(), lost.as_tip()),
            ClientMsg::Forward(chain[3].clone(), lost.as_tip()),
        ]
    );

    setup.send_backward(BRANCH_47);
    setup.send_validated(&chain[7].hash().to_string());
    setup.send_validated(&chain[8].hash().to_string());
    setup.send_validated(&chain[9].hash().to_string());

    let msgs = client.recv_until_await();
    assert_eq!(
        msgs,
        [
            ClientMsg::Forward(chain[4].clone(), chain[9].as_tip()),
            ClientMsg::Forward(chain[5].clone(), chain[9].as_tip()),
            ClientMsg::Forward(chain[6].clone(), chain[9].as_tip()),
            ClientMsg::Forward(chain[7].clone(), chain[9].as_tip()),
            ClientMsg::Forward(chain[8].clone(), chain[9].as_tip()),
            ClientMsg::Forward(chain[9].clone(), chain[9].as_tip()),
        ]
    );
}
