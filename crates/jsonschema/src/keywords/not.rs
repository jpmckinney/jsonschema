use crate::{
    compiler, error::ValidationError, keywords::CompilationResult, node::SchemaNode,
    paths::LazyLocation, validator::Validate,
};
use serde_json::{Map, Value};

pub(crate) struct NotValidator {
    // needed only for error representation
    original: Value,
    node: Box<SchemaNode>,
}

impl NotValidator {
    #[inline]
    pub(crate) fn compile<'a>(ctx: &compiler::Context, schema: &'a Value) -> CompilationResult<'a> {
        let ctx = ctx.new_at_location("not");
        Ok(NotValidator {
            original: schema.clone(),
            node: Box::new(compiler::compile(&ctx, ctx.as_resource_ref(schema))?),
        }
        .into())
    }
}

impl Validate for NotValidator {
    fn is_valid(&self, instance: &Value) -> bool {
        !self.node.is_valid(instance)
    }

    fn validate<'i>(
        &self,
        instance: &'i Value,
        location: &LazyLocation,
    ) -> Result<(), ValidationError<'i>> {
        if self.is_valid(instance) {
            Ok(())
        } else {
            Err(ValidationError::not(
                self.node.location().clone(),
                location.into(),
                instance,
                self.original.clone(),
            ))
        }
    }
}

#[inline]
pub(crate) fn compile<'a>(
    ctx: &compiler::Context,
    _: &'a Map<String, Value>,
    schema: &'a Value,
) -> Option<CompilationResult<'a>> {
    Some(NotValidator::compile(ctx, schema))
}

#[cfg(test)]
mod tests {
    use crate::tests_util;
    use serde_json::json;

    #[test]
    fn location() {
        tests_util::assert_schema_location(
            &json!({"not": {"type": "string"}}),
            &json!("foo"),
            "/not",
        )
    }
}
