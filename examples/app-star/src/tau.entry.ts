// VISION FIXTURE — target state, not yet buildable.
//
// The [synth] entry. tau evaluates THIS file (and only its import graph)
// in the synth sandbox at build time — no network, fs read-only. The
// explicit imports are the collection convention (worked-examples B2):
// the sandbox's read set derives from this graph, and a pipeline module
// not imported here does not exist.

import "./triage/triage.tau";
