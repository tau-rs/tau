// VISION FIXTURE — target state, not yet buildable.
// A deliberately boring Fastify app: tau adds one route dependency and
// zero framework concepts (invariant 12 — rungs never tax).

import Fastify from "fastify";
import { ticketRoutes } from "./routes/tickets";

const app = Fastify();
ticketRoutes(app);
app.listen({ port: 3000 });
