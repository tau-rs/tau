// VISION FIXTURE — target state, not yet buildable.
//
// The caller: ordinary app code. The only tau concept that reaches this
// file is the typed handle. Transport (warm `tau serve` socket, CLI
// NDJSON fallback) is the client's concern, not this file's
// (worked-examples B4; invariant 6 — no tau runtime library in-process).

import type { FastifyInstance } from "fastify";
import { triage } from "../triage/triage.tau";

export function ticketRoutes(app: FastifyInstance) {
  app.post("/tickets", async (req, reply) => {
    const { verdict } = await triage.run({ ticket: req.body }); // typed I/O
    reply.send(verdict);
  });
}
