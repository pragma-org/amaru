use risc0_zkvm::guest::env;
use amaru_examples_shared::forward_ledger;

fn main() {
    // read the input
    let input: String = env::read();

    forward_ledger(&input);

    // write public output to the journal
    env::commit(&input);
}
