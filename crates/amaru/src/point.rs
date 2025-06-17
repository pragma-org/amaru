use amaru_kernel::Point;

/// Convert an Amaru's point into a pallas_network's Point.
///
/// TODO: Remove this function by moving the 'Point' type definition downstream from pallas_network
/// to pallas_primitives.
pub(crate) fn to_network_point(point: Point) -> pallas_network::miniprotocols::Point {
    match point {
        Point::Origin => pallas_network::miniprotocols::Point::Origin,
        Point::Specific(slot, hash) => pallas_network::miniprotocols::Point::Specific(slot, hash),
    }
}

/// Convert a pallas_network's Point into an Amaru's Point.
///
/// TODO: Remove this function by moving the 'Point' type definition downstream from pallas_network
/// to pallas_primitives.
pub(crate) fn from_network_point(point: &pallas_network::miniprotocols::Point) -> Point {
    match point.clone() {
        pallas_network::miniprotocols::Point::Origin => Point::Origin,
        pallas_network::miniprotocols::Point::Specific(slot, hash) => Point::Specific(slot, hash),
    }
}
