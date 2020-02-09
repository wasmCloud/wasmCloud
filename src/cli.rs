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
use crate::jwt::TokenValidation;
use crate::jwt::WascapEntity;
use crate::jwt::{Actor, Claims};
use serde::de::DeserializeOwned;
use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table, TableStyle,
};

impl Claims<Actor> {
    pub fn render(&self, validation: TokenValidation) -> String {
        let mut table = render_core(self, validation);

        let md = self.metadata.clone().unwrap();
        let friendly_rev = md.rev.unwrap_or(0);
        let friendly_ver = md.ver.unwrap_or_else(|| "None".to_string());
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

        let friendly_caps: Vec<String> = if let Some(caps) = &self.metadata.as_ref().unwrap().caps {
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

        let tags = if let Some(tags) = &self.metadata.as_ref().unwrap().tags {
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

        table.render()
    }
}

// * - we don't need render impls for Operator or Account because those tokens are never embedded into a module,
// only actors.

fn token_label(pk: &str) -> String {
    match pk.chars().nth(0).unwrap() {
        'A' => "Account".to_string(),
        'M' => "Module".to_string(),
        'O' => "Operator".to_string(),
        'S' => "Server".to_string(),
        'U' => "User".to_string(),
        _ => "<Unknown>".to_string(),
    }
}

fn render_core<T>(claims: &Claims<T>, validation: TokenValidation) -> Table
where
    T: serde::Serialize + DeserializeOwned + WascapEntity,
{
    let mut table = Table::new();
    table.max_column_width = 68;
    table.style = TableStyle::extended();
    let headline = format!("{} - {}", claims.name(), token_label(&claims.subject));

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        headline,
        2,
        Alignment::Center,
    )]));

    table.add_row(Row::new(vec![
        TableCell::new(token_label(&claims.issuer)),
        TableCell::new_with_alignment(&claims.issuer, 1, Alignment::Right),
    ]));
    table.add_row(Row::new(vec![
        TableCell::new(token_label(&claims.subject)),
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

    table
}
