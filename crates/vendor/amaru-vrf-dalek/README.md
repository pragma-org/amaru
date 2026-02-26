# Verifiable Random Function
**DISCLAIMER**: this crate is under active development and should not be used.

Implementation of the verifiable random function presented in 
[draft-irtf-cfrg-vrf-03](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-vrf-03) using
Edwards25519, SHA512, and Elligator2, and that presented in
[draft-irtf-cfrg-vrf-10](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-vrf-10) using
Edwards25519, SHA512, and Elligator2.

The goal of this crate is to  a compatible implementation with 
the VRF-03 [implemented over libsodium](https://github.com/input-output-hk/libsodium/tree/draft-irtf-cfrg-vrf-03/src/libsodium),
with the latest version of the standard, and with the batch-compatible 
version of the VRF, as presented in this [technical spec](https://iohk.io/en/research/library/papers/on-uc-secure-range-extension-and-batch-verification-for-ecvrf/).

#### Note on compatibility: 
Currently, the tests pass because we are using a [forked curve25519-dalek](https://github.com/iquerejeta/curve25519-dalek)
crate. The implementation of the vrf over libsodium differs in the elligator2
function. `curve25519-dalek`'s API does not allow us to modify the elligator2 
function, which makes use rely on a fork. In particular, [here](https://github.com/input-output-hk/libsodium/blob/draft-irtf-cfrg-vrf-03/src/libsodium/crypto_vrf/ietfdraft03/convert.c#L84)
we clear the sign bit, when it should be cleared only [here](https://github.com/input-output-hk/libsodium/blob/draft-irtf-cfrg-vrf-03/src/libsodium/crypto_core/ed25519/ref10/ed25519_ref10.c#L2527)
(according to the latest standards).
This does not reduce the security of the scheme, but makes it incompatible with other
implementations. 

Similarly, the implementation of the `hash_to_curve` implementation in [curve25519-dalek](https://github.com/dalek-cryptography/curve25519-dalek/pull/377)
is not compatible with the current version of the standard. This forces us to currently
stick to the `try-and-increment` version of the H2C function. While it provides the 
same security guarantees and efficiency, it does not provide compatibility with 
implementations that use the elligator function. We hope to the PR linked above, 
merged soon. 

## Benchmarks
We ran our benchmarks using `RUSTFLAGS='-C target-cpu=native' cargo bench` with 
an `Intel Core i7 @ 2,7 GHz`. We run the benchmarks with and without the feature
`batch_deterministic`.

Using deterministic batching
```
VRF10/Generation        time:   [151.86 us 154.50 us 157.65 us]
VRF10/Verification      time:   [112.43 us 114.26 us 116.30 us]

VRF10 Batch Compat/Generation                                                                             
                        time:   [167.07 us 167.78 us 168.65 us]
VRF10 Batch Compat/Single Verification                                                                             
                        time:   [117.46 us 117.90 us 118.57 us]
VRF10 Batch Compat/Batch Verification/2                                                                             
                        time:   [209.31 us 209.65 us 210.11 us]
VRF10 Batch Compat/Batch Verification/4                                                                            
                        time:   [371.98 us 372.49 us 373.16 us]
VRF10 Batch Compat/Batch Verification/8                                                                            
                        time:   [691.76 us 693.18 us 694.88 us]
VRF10 Batch Compat/Batch Verification/16                                                                            
                        time:   [1.3509 ms 1.3532 ms 1.3562 ms]
VRF10 Batch Compat/Batch Verification/32                                                                            
                        time:   [2.6405 ms 2.6447 ms 2.6503 ms]
VRF10 Batch Compat/Batch Verification/64                                                                            
                        time:   [4.9783 ms 4.9845 ms 4.9920 ms]
VRF10 Batch Compat/Batch Verification/128                                                                             
                        time:   [9.3936 ms 9.4085 ms 9.4276 ms]
VRF10 Batch Compat/Batch Verification/256                                                                             
                        time:   [17.669 ms 17.712 ms 17.764 ms]
VRF10 Batch Compat/Batch Verification/512                                                                            
                        time:   [34.347 ms 34.552 ms 34.772 ms]
VRF10 Batch Compat/Batch Verification/1024                                                                            
                        time:   [66.427 ms 66.553 ms 66.700 ms]
```

Using random batching
```
VRF10 Batch Compat/Single Verification                                                                             
                        time:   [117.02 us 117.19 us 117.40 us]
VRF10 Batch Compat/Batch Verification/2                                                                             
                        time:   [189.38 us 189.77 us 190.28 us]
VRF10 Batch Compat/Batch Verification/4                                                                            
                        time:   [331.81 us 335.83 us 343.93 us]
VRF10 Batch Compat/Batch Verification/8                                                                            
                        time:   [623.53 us 624.47 us 625.58 us]
VRF10 Batch Compat/Batch Verification/16                                                                            
                        time:   [1.2093 ms 1.2146 ms 1.2219 ms]
VRF10 Batch Compat/Batch Verification/32                                                                            
                        time:   [2.2665 ms 2.2708 ms 2.2759 ms]
VRF10 Batch Compat/Batch Verification/64                                                                            
                        time:   [4.2367 ms 4.2446 ms 4.2542 ms]
VRF10 Batch Compat/Batch Verification/128                                                                             
                        time:   [7.9080 ms 7.9315 ms 7.9585 ms]
VRF10 Batch Compat/Batch Verification/256                                                                             
                        time:   [14.653 ms 14.676 ms 14.702 ms]
VRF10 Batch Compat/Batch Verification/512                                                                             
                        time:   [28.060 ms 28.108 ms 28.165 ms]
VRF10 Batch Compat/Batch Verification/1024                                                                            
                        time:   [54.467 ms 54.584 ms 54.711 ms]
```


Translated into cost of a single verification (time in us)

|Size| Deterministic | Non-deterministic | 
|:----: | :----: | :----: |
|1 | 118.88 | 118.88 |
|32 | 89 |75 |
| 64 | 84 | 69 |
|128 | 74 | 69| 
| 256 | 70 | 62 |
| 512 | 69 | 58|
| 1024 | 67 | 56 |

Using non-deterministic batching we can reduce to 0.6 the time per verification
with batches of 64, and 0.47 with batches of 1024. Using deterministic batching
the times are slightly worse, as we need to compute two additional hashes for each 
proof verified. We reduce the time per verification to 0.71 with batches of 64 
and up to 0.56 with batches of 1024. 

Now we print the time it takes to compute exclusively the multi-scalar operation.

Deterministic 
```

VRF10 Batch Compat/Multiscalar multiplication (no insertion)/2                                                                             
                        time:   [119.49 us 119.74 us 120.01 us]
VRF10 Batch Compat/Multiscalar multiplication (no insertion)/4                                                                             
                        time:   [195.84 us 197.03 us 198.70 us]
VRF10 Batch Compat/Multiscalar multiplication (no insertion)/8                                                                            
                        time:   [378.70 us 379.48 us 380.38 us]
VRF10 Batch Compat/Multiscalar multiplication (no insertion)/16                                                                            
                        time:   [721.83 us 722.97 us 724.23 us]
VRF10 Batch Compat/Multiscalar multiplication (no insertion)/32                                                                            
                        time:   [1.4040 ms 1.4063 ms 1.4092 ms]
VRF10 Batch Compat/Multiscalar multiplication (no insertion)/64                                                                            
                        time:   [2.3390 ms 2.3431 ms 2.3476 ms]
VRF10 Batch Compat/Multiscalar multiplication (no insertion)/128                                                                            
                        time:   [4.0649 ms 4.0912 ms 4.1274 ms]
VRF10 Batch Compat/Multiscalar multiplication (no insertion)/256                                                                             
                        time:   [7.1750 ms 7.1892 ms 7.2051 ms]
VRF10 Batch Compat/Multiscalar multiplication (no insertion)/512                                                                             
                        time:   [12.885 ms 12.922 ms 12.965 ms]
VRF10 Batch Compat/Multiscalar multiplication (no insertion)/1024                                                                             
                        time:   [24.290 ms 24.333 ms 24.383 ms]
```

