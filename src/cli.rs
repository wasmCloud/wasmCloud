// Copyright 2015-2018 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and 
// limitations under the License.

use nkeys::KeyPair;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table, TableStyle,
};
use crate::caps::*;
use crate::jwt::{Claims, validate_token};
use crate::Result;

use chrono::prelude::*;
use chrono::NaiveDateTime;
use crate::wasm::embed_claims;

pub fn emit_claims(claims: &Claims, token: &str) {
    let vres = validate_token(token);


    if !vres.is_ok() { 
        println!("Token validation warning: {}", vres.unwrap_err());
        return;
    }

    let validation = vres.unwrap();

    let mut table = Table::new();
    table.max_column_width = 68;
    table.style = TableStyle::extended();

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        "WASCAP Module",
        2,
        Alignment::Center,
    )]));

    table.add_row(Row::new(vec![
        TableCell::new("Account"),
        TableCell::new_with_alignment(&claims.issuer, 1, Alignment::Right),
    ]));
    table.add_row(Row::new(vec![
        TableCell::new("Module"),
        TableCell::new_with_alignment(&claims.subject, 1, Alignment::Right),
    ]));

    table.add_row(Row::new(vec![
        TableCell::new("Expires"),
        TableCell::new_with_alignment(
            validation.expires_human,            
            1,
            Alignment::Right,
        ),
    ]));

    table.add_row(Row::new(vec![
        TableCell::new("Can Be Used"),
        TableCell::new_with_alignment(
            validation.not_before_human,
            1,
            Alignment::Right,
        ),
    ]));
    
    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        "Capabilities",
        2,
        Alignment::Center,
    )]));

    let friendly_caps: Vec<String> = if let Some(caps) = &claims.caps {
        caps.iter().map(|c| capability_name(&c)).collect()
    } else {
        vec![]
    };

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        friendly_caps.join("\n"),
        2,
        Alignment::Left,
    )]));

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        "Tags",
        2,
        Alignment::Center,
    )]));

    let tags = if let Some(tags) = &claims.tags {
        if tags.len() == 0 {
            "None".to_string()
        } else {
            tags.join(",")
        }
    } else {
        "None".to_string()
    };
    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        tags,
        2,
        Alignment::Left,
    )]));

    println!("{}", table.render());
}


#[cfg(test)]
mod test {
    use super::{since_the_epoch, stamp_to_human, SECS_PER_DAY};

    #[test]
    fn friendly_relative_time() {
        let stamp = since_the_epoch().as_secs() + (10 * SECS_PER_DAY);

        let f = stamp_to_human(stamp as i64);
        assert_eq!("in a week", f);

        let stamp2 = since_the_epoch().as_secs() - (10 * SECS_PER_DAY);
        let f2 = stamp_to_human(stamp2 as i64);
        assert_eq!("a week ago", f2);
    }
}