//! Public tracked point routing followed by actual immutable input adoption.
use super::{GenerationRoute, Generations, Input, InputRoute, Inputs, Point};
use crate::domain::WordElement;
use analysis_kernel::{
    Answer, Availability, Discover, GenerationEntry, Read, ReadGeneration, Scope, Scoped,
};

/// Require another point's actual current native input on the shared routes.
/// Unknown points and stale known producers remain mandatory Pending answers.
/// Retain the Point and resolve it again after a wake; a historical Subject is
/// not a live selector. Value never certifies complete predecessor discovery.
pub fn current_input<N, Cx>(
    cx: &mut Cx,
    universe: Scope,
    point: Point,
) -> Result<Answer<Input<N>>, <Cx as Read<Generations, GenerationRoute>>::Error>
where
    N: WordElement,
    Cx: Read<Generations, GenerationRoute>
        + ReadGeneration<Error = <Cx as Read<Generations, GenerationRoute>>::Error>
        + Read<Inputs<N>, InputRoute, Error = <Cx as Read<Generations, GenerationRoute>>::Error>,
{
    let entry =
        <Cx as Read<Generations, GenerationRoute>>::query(cx, Scoped::new(universe, point))?;
    selected_input(cx, entry)
}

/// Explore an as-yet unknown point without blocking its own CFG discovery.
/// Only the initial directory lookup is optional. Once a producer exists,
/// current-generation validity and its actual Input are mandatory tracked reads;
/// discovery cannot make a stale guarantee available or prove unreachability.
pub fn discover_input<N, Cx>(
    cx: &mut Cx,
    universe: Scope,
    point: Point,
) -> Result<Answer<Input<N>>, <Cx as Discover<Generations, GenerationRoute>>::Error>
where
    N: WordElement,
    Cx: Discover<Generations, GenerationRoute>
        + ReadGeneration<Error = <Cx as Discover<Generations, GenerationRoute>>::Error>
        + Read<Inputs<N>, InputRoute, Error = <Cx as Discover<Generations, GenerationRoute>>::Error>,
{
    let entry =
        <Cx as Discover<Generations, GenerationRoute>>::discover(cx, Scoped::new(universe, point))?;
    selected_input(cx, entry)
}

fn selected_input<N, Cx>(
    cx: &mut Cx,
    entry: Answer<GenerationEntry>,
) -> Result<Answer<Input<N>>, <Cx as ReadGeneration>::Error>
where
    N: WordElement,
    Cx: ReadGeneration + Read<Inputs<N>, InputRoute, Error = <Cx as ReadGeneration>::Error>,
{
    let generation = match entry.state {
        Availability::Value(generation) => generation,
        Availability::Pending => {
            return Ok(Answer {
                state: Availability::Pending,
                version: entry.version,
                scope: entry.scope,
            });
        }
        Availability::Absent => {
            return Ok(Answer {
                state: Availability::Absent,
                version: entry.version,
                scope: entry.scope,
            });
        }
    };
    let selected = cx.current_generation(&generation)?;
    match selected.state {
        Availability::Value(scope) => {
            <Cx as Read<Inputs<N>, InputRoute>>::query(cx, Scoped::new(scope, ()))
        }
        Availability::Pending => Ok(Answer {
            state: Availability::Pending,
            version: selected.version,
            scope: selected.scope,
        }),
        Availability::Absent => Ok(Answer {
            state: Availability::Absent,
            version: selected.version,
            scope: selected.scope,
        }),
    }
}
