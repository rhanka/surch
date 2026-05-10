export type EngineId = 'surch' | 'opensearch';
export type DemoMode = EngineId | 'compare';
export type QueryId = 'count' | 'match_label' | 'bool_address' | 'fuzzy_label';

export type EngineConfig = {
  id: EngineId;
  label: string;
  url: string;
  configured: boolean;
};

export type BanDocument = {
  city_code: string;
  city_name: string;
  house_number: string;
  id: string;
  label: string;
  location: {
    lat: number;
    lon: number;
  };
  postcode: string;
  source: 'BAN';
  street_name: string;
};

export type BanSourceProfile = {
  kind: 'tiny' | 'csv';
  name: string;
  offline: boolean;
  bounded: boolean;
  path?: string;
  officialUrl?: string;
  downloadCommand?: string;
};

export type BanDatasetSummary = {
  name: string;
  documentCount: number;
  officialSource?: string;
};

export type FixtureOperation = {
  kind: 'create_index' | 'bulk' | 'refresh';
  path: string;
  body?: string;
  expected_status: number;
};

export type BanFixture = {
  name: 'ban_tiny';
  description: string;
  operations: FixtureOperation[];
  documents: BanDocument[];
};

export type DemoQuery = {
  id: QueryId;
  label: string;
  kind: 'count' | 'search';
  expectedId: string | null;
  body?: unknown;
};

export type EngineOperationResult = {
  path: string;
  status: number;
};

export type EngineResponse = {
  engine: EngineId;
  latencyMs?: number;
  status: number;
  response: unknown;
};
