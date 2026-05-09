import { json } from '@sveltejs/kit';
import { getBanTinyFixture } from '$lib/server/fixture';
import type { RequestHandler } from './$types';

export const GET = (() => {
  const fixture = getBanTinyFixture();

  return json({
    status: 'ok',
    dataset: fixture.name,
    documentCount: fixture.documents.length
  });
}) satisfies RequestHandler;
