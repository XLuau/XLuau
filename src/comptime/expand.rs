use crate::{
    ast::Program,
    compiler::Result,
};

pub fn expand_program(program: &Program) -> Result<Program> {
    Ok(program.clone())
}
