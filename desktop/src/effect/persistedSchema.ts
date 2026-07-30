import { Schema } from "effect";

// Persisted desktop data has accumulated several historical shapes. These
// schemas validate only the outer object boundary; field-level migration and
// filtering intentionally remain in profileModel's normalizers.
export const PersistedRunnerSettingsSchema = Schema.Struct({
  binaryPath: Schema.optionalKey(Schema.Unknown),
  env: Schema.optionalKey(Schema.Unknown),
});

export const PersistedProfileStateSchema = Schema.Struct({
  activeProfileId: Schema.optionalKey(Schema.Unknown),
  profiles: Schema.optionalKey(Schema.Unknown),
  openProfileIds: Schema.optionalKey(Schema.Unknown),
});

export const decodePersistedRunnerSettings = Schema.decodeUnknownResult(
  PersistedRunnerSettingsSchema,
);
export const decodePersistedProfileState = Schema.decodeUnknownResult(
  PersistedProfileStateSchema,
);
