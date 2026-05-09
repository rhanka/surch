export const MAX_BAN_SUGGESTIONS = 25;
export const DEFAULT_BAN_SUGGESTIONS = 8;
export const MAX_EXTERNAL_BAN_ROWS = 25_000;

export type BanRepositoryEnv = {
  BAN_CSV_PATH?: string;
  BAN_DATA_DIR?: string;
  BAN_SAMPLE_LIMIT?: string;
};
