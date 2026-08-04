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

mod amaru;
mod cardano;
mod config;
mod splash;

pub(super) use self::{
    amaru::{page_content_height as amaru_page_content_height, render_amaru},
    cardano::{page_content_height as cardano_page_content_height, render_cardano},
    config::{page_content_height as config_page_content_height, render_config},
    splash::render_splash,
};
