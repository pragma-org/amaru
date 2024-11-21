use amaru::sync::Point;

pub(crate) mod bootstrap;
pub(crate) mod daemon;

pub(crate) fn parse_point<'a, F, E>(raw_str: &str, bail: F) -> Result<Point, E>
where
    F: Fn(&'a str) -> E + 'a,
{
    let mut split = raw_str.split('.');

    let slot = split
        .next()
        .ok_or(bail("missing slot number before '.'"))
        .and_then(|s| {
            s.parse::<u64>()
                .map_err(|_| bail("failed to parse point's slot as a non-negative integer"))
        })?;

    let block_header_hash = split
        .next()
        .ok_or(bail("missing block header hash after '.'"))
        .and_then(|s| {
            hex::decode(s).map_err(|_| bail("unable to decode block header hash from hex"))
        })?;

    Ok(Point::Specific(slot, block_header_hash))
}
