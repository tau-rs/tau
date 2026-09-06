// VISION FIXTURE — target state, not yet runnable.
//
// The caller: ordinary app code. The only tau surface at runtime is the
// generated client — definitions go INTO the toolchain, tau/gen comes
// OUT, and the app only touches what comes out. pipelines.triage is a
// generated typed symbol (never a string id); run() executes the pinned,
// frozen artifact out-of-process (worked-examples B3/B4; invariant 6).

import type { FastifyInstance } from "fastify";
import { pipelines } from "../../tau/gen";

export function ticketRoutes(app: FastifyInstance) {
  app.post("/tickets", async (req, reply) => {
    const { verdict } = await pipelines.triage.run({ ticket: req.body });
    reply.send(verdict);
  });
}
