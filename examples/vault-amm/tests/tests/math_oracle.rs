//! Replays the vectors produced by `scripts/gen_vectors.py` (Python big
//! integers, no shared code with the contract) against `math.rs`.

use avm_vault_amm::math::{
    mul_div_ceil, mul_div_floor, sqrt_product, swap_in, swap_out, PriceError,
};

const VECTORS: &str = include_str!("../vectors/math.tsv");

fn number(text: &str) -> u128 {
    text.parse().unwrap_or_else(|_| panic!("bad vector number {text}"))
}

fn show(value: Option<u128>) -> String {
    value.map_or_else(|| "none".to_string(), |v| v.to_string())
}

fn price(result: Result<u128, PriceError>) -> String {
    match result {
        Ok(value) => value.to_string(),
        Err(PriceError::InvalidInput) => "invalid".to_string(),
        Err(PriceError::InsufficientLiquidity) => "liquidity".to_string(),
        Err(PriceError::Overflow) => "overflow".to_string(),
    }
}

#[test]
fn every_oracle_vector_is_reproduced_exactly() {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for line in VECTORS.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let (op, expected) = (fields[0], fields[fields.len() - 1]);
        let args: Vec<u128> = fields[1..fields.len() - 1].iter().map(|f| number(f)).collect();
        let actual = match op {
            "mul_div_floor" => show(mul_div_floor(args[0], args[1], args[2])),
            "mul_div_ceil" => show(mul_div_ceil(args[0], args[1], args[2])),
            "sqrt_product" => sqrt_product(args[0], args[1]).to_string(),
            "swap_out" => price(swap_out(args[0], args[1], args[2], args[3] as u16)),
            "swap_in" => price(swap_in(args[0], args[1], args[2], args[3] as u16)),
            other => panic!("unknown vector op {other}"),
        };
        assert_eq!(actual, expected, "{line}");
        *counts.entry(op).or_default() += 1;
    }
    assert_eq!(counts.len(), 5, "every operation must be covered: {counts:?}");
    assert!(counts.values().all(|n| *n >= 350), "{counts:?}");
}

#[test]
fn the_vectors_exercise_every_failure_kind() {
    for kind in ["\tnone", "\tinvalid", "\tliquidity", "\toverflow"] {
        assert!(
            VECTORS.lines().any(|line| line.ends_with(kind)),
            "no vector ends with {kind:?}; the oracle would not catch regressions there"
        );
    }
}
