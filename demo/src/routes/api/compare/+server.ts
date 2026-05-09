import { json } from '@sveltejs/kit';
import { compareBanQuery, parseQueryId } from '$lib/server/engines';
import { toDemoErrorResponse } from '$lib/server/demoErrors';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ request, fetch }) => {
  const body = await request.json().catch(() => ({}));
  const queryId = parseQueryId(body.queryId ?? 'match_label');

  try {
    return json(await compareBanQuery(queryId, fetch));
  } catch (error) {
    const formatted = toDemoErrorResponse(error);

    if (formatted) {
      return json(formatted.body, { status: formatted.status });
    }

    throw error;
  }
};
