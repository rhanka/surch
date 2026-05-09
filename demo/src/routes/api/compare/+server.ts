import { json } from '@sveltejs/kit';
import { compareBanQuery, parseQueryId } from '$lib/server/engines';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ request, fetch }) => {
  const body = await request.json().catch(() => ({}));
  const queryId = parseQueryId(body.queryId ?? 'match_label');

  return json(await compareBanQuery(queryId, fetch));
};
