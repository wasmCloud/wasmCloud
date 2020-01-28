// Copyright 2015-2019 Capital One Services, LLC
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

use crate::caps::*;
use crate::jwt::{validate_token, Claims};
use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table, TableStyle,
};

pub fn emit_claims(claims: &Claims, token: &str) {
    if claims.metadata.is_none(){

        println!("No metadata found in JWT claim.");
        return;
    }
    let vres = validate_token(token);

    if let Err(e) = vres {
        println!("Token validation warning: {}", e);
        return;
    }

    let validation = vres.unwrap();

    let mut table = Table::new();
    table.max_column_width = 68;
    table.style = TableStyle::extended();
    let headline = format!(
        "Secure {} Module",
        if claims.metadata.as_ref().unwrap().provider {
            "Capability Provider"
        } else {
            "Actor"
        }
    );

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        headline,
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
        TableCell::new_with_alignment(validation.expires_human, 1, Alignment::Right),
    ]));

    table.add_row(Row::new(vec![
        TableCell::new("Can Be Used"),
        TableCell::new_with_alignment(validation.not_before_human, 1, Alignment::Right),
    ]));

    let md = claims.metadata.clone().unwrap();
    let friendly_rev = md.rev.unwrap_or(0);
    let friendly_ver = md.ver.unwrap_or("None".to_string());
    let friendly = format!("{} ({})", friendly_ver, friendly_rev);
    
    table.add_row(Row::new(vec![
        TableCell::new("Version"),
        TableCell::new_with_alignment(friendly, 1, Alignment::Right),
    ]));

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        "Capabilities",
        2,
        Alignment::Center,
    )]));

    let friendly_caps: Vec<String> = if let Some(caps) = &claims.metadata.as_ref().unwrap().caps {
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

    let tags = if let Some(tags) = &claims.metadata.as_ref().unwrap().tags {
        if tags.is_empty() {
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
