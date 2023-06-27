//! So we don't polute the main lib with debug impls

use crate::{Actor, Envelope};

impl<A> std::fmt::Debug for Envelope<A>
where
    A: Actor,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("message", &"{{..}}")
            .finish()
    }
}
