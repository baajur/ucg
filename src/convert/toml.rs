// Copyright 2018 Jeremy Wall
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

use std;
use std::error;
use std::io::Write;
use std::rc::Rc;

use simple_error::SimpleError;
use toml;

use ast;
use build::Val;
use convert::traits::{Converter, Result};

pub struct TomlConverter {}

type ConvertResult = std::result::Result<toml::Value, Box<error::Error>>;

impl TomlConverter {
    pub fn new() -> Self {
        TomlConverter {}
    }

    fn convert_list(&self, items: &Vec<Rc<Val>>) -> ConvertResult {
        let mut v = Vec::new();
        for val in items.iter() {
            v.push(try!(self.convert_value(val)));
        }
        Ok(toml::Value::Array(v))
    }

    fn convert_tuple(&self, items: &Vec<(ast::PositionedItem<String>, Rc<Val>)>) -> ConvertResult {
        let mut mp = toml::value::Table::new();
        for &(ref k, ref v) in items.iter() {
            mp.entry(k.val.clone())
                .or_insert(try!(self.convert_value(v)));
        }
        Ok(toml::Value::Table(mp))
    }

    fn convert_value(&self, v: &Val) -> ConvertResult {
        let toml_val = match v {
            &Val::Boolean(b) => toml::Value::Boolean(b),
            // TODO(jwall): This is an error apparently
            &Val::Empty => {
                let err = SimpleError::new("Nulls are not allowed in Toml Conversions!");
                return Err(Box::new(err));
            }
            &Val::Float(f) => toml::Value::Float(f),
            &Val::Int(i) => toml::Value::Integer(i),
            &Val::Str(ref s) => toml::Value::String(s.clone()),
            &Val::Macro(_) => {
                let err = SimpleError::new("Macros are not allowed in Toml Conversions!");
                return Err(Box::new(err));
            }
            &Val::Module(_) => {
                let err = SimpleError::new("Modules are not allowed in Toml Conversions!");
                return Err(Box::new(err));
            }
            &Val::List(ref l) => try!(self.convert_list(l)),
            &Val::Tuple(ref t) => try!(self.convert_tuple(t)),
        };
        Ok(toml_val)
    }

    fn write(&self, v: &Val, w: &mut Write) -> Result {
        let toml_val = try!(self.convert_value(v));
        let toml_bytes = try!(toml::ser::to_string_pretty(&toml_val));
        try!(write!(w, "{}", toml_bytes));
        Ok(())
    }
}

impl Converter for TomlConverter {
    fn convert(&self, v: Rc<Val>, mut w: &mut Write) -> Result {
        self.write(&v, &mut w)
    }

    fn file_ext(&self) -> String {
        String::from("toml")
    }

    fn description(&self) -> String {
        "Convert ucg Vals into valid ucg.".to_string()
    }
}
