// Copyright 2026 PRAGMA
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

use amaru_ouroboros_traits::StoreError;
use rocksdb::{
    DB, DBCommon, DBIteratorWithThreadMode, DBPinnableSlice, IteratorMode, ReadOptions, SnapshotWithThreadMode,
};

pub trait DbOps: Sized {
    type Iter<'a>: Iterator<Item = Result<(Box<[u8]>, Box<[u8]>), rocksdb::Error>>
    where
        Self: 'a;

    fn get_pinned(&self, key: &[u8], opts: ReadOptions) -> Result<Option<DBPinnableSlice<'_>>, StoreError>;
    fn multi_get(&self, keys: &[&[u8]], opts: ReadOptions) -> Vec<Result<Option<Vec<u8>>, StoreError>>;
    fn iterator_opt<'a>(&'a self, mode: IteratorMode<'_>, opts: ReadOptions) -> Self::Iter<'a>;
}

impl DbOps for DB {
    type Iter<'a>
        = DBIteratorWithThreadMode<'a, Self>
    where
        Self: 'a;

    fn get_pinned(&self, key: &[u8], opts: ReadOptions) -> Result<Option<DBPinnableSlice<'_>>, StoreError> {
        DBCommon::get_pinned_opt(self, key, &opts).map_err(|e| StoreError::ReadError { error: e.to_string() })
    }

    fn multi_get(&self, keys: &[&[u8]], opts: ReadOptions) -> Vec<Result<Option<Vec<u8>>, StoreError>> {
        DBCommon::multi_get_opt(self, keys, &opts)
            .into_iter()
            .map(|result| result.map_err(|e| StoreError::ReadError { error: e.to_string() }))
            .collect()
    }

    fn iterator_opt<'a>(&'a self, mode: IteratorMode<'_>, opts: ReadOptions) -> Self::Iter<'a> {
        DBCommon::iterator_opt(self, mode, opts)
    }
}

impl DbOps for SnapshotWithThreadMode<'_, DB> {
    type Iter<'a>
        = DBIteratorWithThreadMode<'a, DB>
    where
        Self: 'a;

    fn get_pinned(&self, key: &[u8], opts: ReadOptions) -> Result<Option<DBPinnableSlice<'_>>, StoreError> {
        self.get_pinned_opt(key, opts).map_err(|e| StoreError::ReadError { error: e.to_string() })
    }

    fn multi_get(&self, keys: &[&[u8]], opts: ReadOptions) -> Vec<Result<Option<Vec<u8>>, StoreError>> {
        self.multi_get_opt(keys, opts)
            .into_iter()
            .map(|result| result.map_err(|e| StoreError::ReadError { error: e.to_string() }))
            .collect()
    }

    fn iterator_opt<'a>(&'a self, mode: IteratorMode<'_>, opts: ReadOptions) -> Self::Iter<'a> {
        SnapshotWithThreadMode::iterator_opt(self, mode, opts)
    }
}
