import type { DemoQuery, QueryId } from '$lib/types';

export const demoQueries: DemoQuery[] = [
  {
    id: 'count',
    label: 'Count all BAN addresses',
    kind: 'count',
    expectedId: null
  },
  {
    id: 'match_label',
    label: 'Match label: Rue de Rivoli',
    kind: 'search',
    expectedId: '75101_0001_00001',
    body: {
      query: {
        match: {
          label: 'Rue de Rivoli'
        }
      }
    }
  },
  {
    id: 'bool_address',
    label: "Bool address: Cours de l'Intendance + 33000",
    kind: 'search',
    expectedId: '33063_0002_00010B',
    body: {
      query: {
        bool: {
          must: [
            {
              match: {
                street_name: "Cours de l'Intendance"
              }
            },
            {
              match: {
                postcode: '33000'
              }
            }
          ]
        }
      }
    }
  },
  {
    id: 'fuzzy_label',
    label: 'Fuzzy typo: Ale des Erables',
    kind: 'search',
    expectedId: '67482_0003_00007',
    body: {
      query: {
        fuzzy: {
          label: {
            value: 'Ale des Erables',
            fuzziness: 2
          }
        }
      }
    }
  }
];

export function getDemoQuery(queryId: QueryId): DemoQuery {
  const query = demoQueries.find((candidate) => candidate.id === queryId);
  if (!query) {
    throw new Error(`unknown demo query ${queryId}`);
  }

  return query;
}
