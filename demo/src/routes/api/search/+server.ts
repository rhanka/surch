import { json } from '@sveltejs/kit';
import { parseEngineId, parseQueryId, runBanQuery } from '$lib/server/engines';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ request, fetch }) => {
  const body = await request.json().catch(() => ({}));
  const engine = parseEngineId(body.engine ?? 'surch');
  const queryId = parseQueryId(body.queryId ?? 'match_label');

  return json(await runBanQuery({ engine, queryId }, fetch));
};
