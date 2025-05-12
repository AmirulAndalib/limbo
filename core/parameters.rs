use super::ast;
use std::num::NonZero;

#[derive(Clone, Debug)]
pub enum Parameter {
    Anonymous(NonZero<usize>),
    Indexed(NonZero<usize>),
    Named(String, NonZero<usize>),
}

impl PartialEq for Parameter {
    fn eq(&self, other: &Self) -> bool {
        self.index() == other.index()
    }
}

impl Parameter {
    pub fn index(&self) -> NonZero<usize> {
        match self {
            Parameter::Anonymous(index) => *index,
            Parameter::Indexed(index) => *index,
            Parameter::Named(_, index) => *index,
        }
    }
}

#[derive(Debug)]
struct InsertContext {
    param_positions: Vec<usize>,
    current_col_value_idx: usize,
}

impl InsertContext {
    fn new(param_positions: Vec<usize>) -> Self {
        Self {
            param_positions,
            current_col_value_idx: 0,
        }
    }

    /// Find the relevant parameter index needed for the current value index of insert stmt
    /// Example for table t (a,b,c):
    ///         `insert into t (c,a,b) values (?,?,?)`
    ///
    /// col a -> value_index 1
    /// col b -> value_index 2
    /// col c -> value_index 0
    ///
    /// however translation will always result in parameters 1, 2, 3
    /// because columns are translated in the table order so `col a` gets
    /// translated first, translate_expr calls parameters.push and always gets index 1.
    ///
    /// Instead, we created an array representing all the value_index's that are type
    /// Expr::Variable, in the case above would be [1, 2, 0], and stored it in insert_ctx.
    /// That array can be used to look up the necessary parameter index by searching for the value
    /// index in the array and returning the index of that value + 1.
    ///  value_index->   [1, 2, 0]
    ///  param index->   |0, 1, 2|
    fn get_insert_param_index(&self) -> Option<NonZero<usize>> {
        self.param_positions
            .iter()
            .position(|param| param.eq(&self.current_col_value_idx))
            .map(|p| NonZero::new(p + 1).unwrap())
    }
}

#[derive(Debug)]
pub struct Parameters {
    index: NonZero<usize>,
    pub list: Vec<Parameter>,
    // Context for reordering parameters during insert statements
    insert_ctx: Option<InsertContext>,
}

impl Default for Parameters {
    fn default() -> Self {
        Self::new()
    }
}

impl Parameters {
    pub fn new() -> Self {
        Self {
            index: 1.try_into().unwrap(),
            list: vec![],
            insert_ctx: None,
        }
    }

    pub fn count(&self) -> usize {
        let mut params = self.list.clone();
        params.dedup();
        params.len()
    }

    /// Begin preparing for an Insert statement by providing the array of values from the Insert body.
    pub fn init_insert_parameters(&mut self, values: &[Vec<ast::Expr>]) {
        self.insert_ctx = Some(InsertContext::new(expected_param_indicies(values)));
    }

    /// Set the value index for the column currently being translated for an Insert stmt.
    pub fn set_insert_value_index(&mut self, idx: usize) {
        if let Some(ctx) = &mut self.insert_ctx {
            ctx.current_col_value_idx = idx;
        }
    }

    pub fn name(&self, index: NonZero<usize>) -> Option<String> {
        self.list.iter().find_map(|p| match p {
            Parameter::Anonymous(i) if *i == index => Some("?".to_string()),
            Parameter::Indexed(i) if *i == index => Some(format!("?{i}")),
            Parameter::Named(name, i) if *i == index => Some(name.to_owned()),
            _ => None,
        })
    }

    pub fn index(&self, name: impl AsRef<str>) -> Option<NonZero<usize>> {
        self.list
            .iter()
            .find_map(|p| match p {
                Parameter::Named(n, index) if n == name.as_ref() => Some(index),
                _ => None,
            })
            .copied()
    }

    pub fn next_index(&mut self) -> NonZero<usize> {
        let index = self.index;
        self.index = self.index.checked_add(1).unwrap();
        index
    }

    pub fn push(&mut self, name: impl AsRef<str>) -> NonZero<usize> {
        match name.as_ref() {
            "" => {
                let index = self.next_index();
                self.list.push(Parameter::Anonymous(index));
                tracing::trace!("anonymous parameter at {index}");
                if let Some(idx) = &self.insert_ctx {
                    idx.get_insert_param_index().unwrap_or(index)
                } else {
                    index
                }
            }
            name if name.starts_with(['$', ':', '@', '#']) => {
                match self
                    .list
                    .iter()
                    .find(|p| matches!(p, Parameter::Named(n, _) if name == n))
                {
                    Some(t) => {
                        let index = t.index();
                        self.list.push(t.clone());
                        tracing::trace!("named parameter at {index} as {name}");
                        index
                    }
                    None => {
                        let index = self.next_index();
                        self.list.push(Parameter::Named(name.to_owned(), index));
                        tracing::trace!("named parameter at {index} as {name}");
                        index
                    }
                }
            }
            index => {
                // SAFETY: Guaranteed from parser that the index is bigger than 0.
                let index: NonZero<usize> = index.parse().unwrap();
                if index > self.index {
                    self.index = index.checked_add(1).unwrap();
                }
                self.list.push(Parameter::Indexed(index));
                tracing::trace!("indexed parameter at {index}");
                index
            }
        }
    }
}

/// Gather all the expected indicies of all Expr::Variable
/// in the provided array of insert values.
pub fn expected_param_indicies(cols: &[Vec<ast::Expr>]) -> Vec<usize> {
    cols.iter()
        .flat_map(|col| col.iter())
        .enumerate()
        .filter(|(_, col)| matches!(col, ast::Expr::Variable(_)))
        .map(|(i, _)| i)
        .collect::<Vec<_>>()
}
