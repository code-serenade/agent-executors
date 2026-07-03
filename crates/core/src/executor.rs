use crate::Result;

#[allow(async_fn_in_trait)]
pub trait Executor {
    type Request;
    type Output;

    async fn execute(&self, request: Self::Request) -> Result<Self::Output>;
}
