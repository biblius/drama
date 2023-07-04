//! So we don't polute the main lib with debug impls

use crate::{Actor, Envelope, Handler};

impl<M, A> std::fmt::Debug for Envelope<M, A>
where
    A: Actor + Handler<M>,
    M: Clone + Send,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("message", &"{{..}}")
            .finish()
    }
}
