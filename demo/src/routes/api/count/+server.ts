import { json } from '@sveltejs/kit';
import { runBanQuery, parseEngineId } from '$lib/server/engines';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ request, fetch }) => {
  const body = await request.json().catch(() => ({}));
  const engine = parseEngineId(body.engine ?? 'surch');

  return json(await runBanQuery({ engine, queryId: 'count' }, fetch));
};
