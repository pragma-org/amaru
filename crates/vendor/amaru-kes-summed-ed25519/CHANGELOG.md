# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (06-03-2023)
### Added
- Feature `sk_clone_enabled` to allow copying the secret key to an array. This should only be used
  for testing or when secure forgetting is not implemented.
- Function `to_pk()` that computes the associated `PublicKey` from a given `KesSk`.

### Changed 
- `KesSk` is no longer represented by an array, but instead by a mutable reference to a slice. This
   forces the implementor to allocate a buffer of bytes that will be used to store the secret key,
   enabling `mlock`ing the buffer.
- Consequently, when generating a key one needs to define a mutable buffer that lives as long as 
  the `KesSk`.

## 0.1.1 (04-01-2023)
### Added
- Function, `get_period()`, to get the current period from a `KesSk`. 

## 0.1.0 (30-12-2022)
Initial release.