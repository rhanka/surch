import { describe, expect, it } from 'vitest';
import { compareBanQuery, resetBanTiny, runBanQuery } from './engines';

describe('engine operations', () => {
  it('resets BAN tiny with fixed OpenSearch-compatible operations', async () => {
    const calls: Array<{ url: string; method: string; body?: string }> = [];
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      calls.push({
        url: url.toString(),
        method: init?.method ?? 'GET',
        body: typeof init?.body === 'string' ? init.body : undefined
      });

      return Response.json({ acknowledged: true });
    };

    const result = await resetBanTiny('surch', fakeFetch);

    expect(result.engine).toBe('surch');
    expect(result.operations).toEqual([
      { path: '/ban_tiny', status: 200 },
      { path: '/_bulk', status: 200 },
      { path: '/ban_tiny/_refresh', status: 200 }
    ]);
    expect(calls.map((call) => [call.method, new URL(call.url).pathname])).toEqual([
      ['PUT', '/ban_tiny'],
      ['POST', '/_bulk'],
      ['POST', '/ban_tiny/_refresh']
    ]);
    expect(calls[1].body).toContain('75101_0001_00001');
  });

  it('runs predefined queries without accepting arbitrary paths', async () => {
    const calls: Array<{ url: string; method: string }> = [];
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      calls.push({
        url: url.toString(),
        method: init?.method ?? 'GET'
      });

      return Response.json({
        hits: {
          total: { value: 1, relation: 'eq' },
          hits: [{ _id: '75101_0001_00001' }]
        }
      });
    };

    const result = await runBanQuery({ engine: 'surch', queryId: 'match_label' }, fakeFetch);
    const response = result.response as { hits: { total: { value: number } } };

    expect(result.engine).toBe('surch');
    expect(result.query.id).toBe('match_label');
    expect(response.hits.total.value).toBe(1);
    expect(calls.map((call) => [call.method, new URL(call.url).pathname])).toEqual([
      ['POST', '/ban_tiny/_search']
    ]);
  });

  it('compares the same query on Surch and OpenSearch', async () => {
    const seen = new Set<string>();
    const fakeFetch = async (url: URL | RequestInfo) => {
      seen.add(new URL(url.toString()).origin);

      return Response.json({
        hits: {
          total: { value: 1, relation: 'eq' },
          hits: [{ _id: '67482_0003_00007' }]
        }
      });
    };

    const result = await compareBanQuery('fuzzy_label', fakeFetch);

    expect(result.query.id).toBe('fuzzy_label');
    expect(result.surch.status).toBe(200);
    expect(result.opensearch.status).toBe(200);
    expect([...seen].sort()).toEqual(['http://127.0.0.1:7700', 'http://127.0.0.1:9200']);
  });
});
