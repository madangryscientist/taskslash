import handler from "@tanstack/react-start/server-entry";

/** The Worker's entry: TanStack Start's `fetch`. A cron or a queue handler goes beside it. */
export default {
  fetch: (request: Request): Response | Promise<Response> => handler.fetch(request),
};
