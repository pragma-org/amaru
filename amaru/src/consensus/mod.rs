pub mod validate;

pub type RawBlock = Vec<u8>;
pub type Point = pallas_network::miniprotocols::Point;

#[derive(Clone)]
pub enum ValidateHeaderEvent {
    Validated(Point, RawBlock),
    Rollback(Point),
}
