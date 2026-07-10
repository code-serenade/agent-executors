use crate::Result;

#[allow(async_fn_in_trait)]
pub trait Executor {
    type Request;
    type Output;

    async fn execute(&self, request: Self::Request) -> Result<Self::Output>;
}

/// Starts an execution that remains alive after the initial call returns.
///
/// Unlike [`Executor`], this trait does not promise a terminal output from `start`.
/// The returned session owns its event and control protocol.
#[allow(async_fn_in_trait)]
pub trait SessionExecutor {
    type Request;
    type Session;

    async fn start(&self, request: Self::Request) -> Result<Self::Session>;
}
