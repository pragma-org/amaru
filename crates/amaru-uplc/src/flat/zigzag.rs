// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::constant::Integer;

pub trait ZigZag {
    type Zag;

    fn zigzag(self) -> Self::Zag;
    fn unzigzag(self) -> Self::Zag;
}

impl ZigZag for &Integer {
    type Zag = Integer;

    fn zigzag(self) -> Self::Zag {
        if self >= &Integer::ZERO {
            // For non-negative numbers, just multiply by 2 (left shift by 1)
            self.clone() << 1
        } else {
            // For negative numbers: -(2 * n) - 1
            // First multiply by 2
            let double: Integer = self.clone() << 1;

            // Then negate and subtract 1
            -double - Integer::from(1)
        }
    }

    fn unzigzag(self) -> Self::Zag {
        let temp: Integer = self.clone() & Integer::from(1);

        (self.clone() >> 1) ^ -(temp)
    }
}
