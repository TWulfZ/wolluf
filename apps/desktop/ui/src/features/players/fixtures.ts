// Test-only typed fixtures mirroring spec 004 AC3 (pilot-shaped, third parties anonymized, R2 counts).
import type { AliasListDto, AliasRowDto, ProfileEntryDto } from "@/ipc/bindings";

let nextId = 0;

function row(rawName: string, nPlays: number, overrides: Partial<AliasRowDto> = {}): AliasRowDto {
  nextId += 1;
  return {
    aliasId: nextId,
    rawName,
    isEmptyName: rawName === "",
    normalizedLength: rawName.replace(/[^\p{L}\p{N}]/gu, "").length,
    nPlays,
    byKeymode: [{ bucket: "k7", n: nPlays }],
    firstPlayedAt: "2025-01-02T10:00:00.000Z",
    lastPlayedAt: "2026-09-27T22:00:00.000Z",
    nOnline: 0,
    nOffline: nPlays,
    nWithReplay: nPlays,
    topCharts: [{ chartMd5: "0123456789abcdef0123456789abcdef", title: "Chart A", version: "7K Hard", n: 3 }],
    autoMatch: null,
    decision: null,
    selected: false,
    inSelfProfile: false,
    ...overrides,
  };
}

/** Rows in the R6 order the backend sends: auto matches first, then play count descending. */
export function pilotAliasList(overrides: Partial<AliasListDto> = {}): AliasListDto {
  nextId = 0;
  const auto = { selected: true, inSelfProfile: true } as const;
  return {
    selectionVersion: 1,
    cfgUsernameAvailable: true,
    wizardNeeded: true,
    aliases: [
      row("TWulfZ", 3019, {
        ...auto,
        autoMatch: { source: "cfg_username", kind: "prefix" },
        nOnline: 270,
        nOffline: 2749,
        byKeymode: [
          { bucket: "k4", n: 19 },
          { bucket: "k7", n: 3000 },
        ],
      }),
      row("TWulfZasdasdasd d jSS||", 27, { ...auto, autoMatch: { source: "cfg_username", kind: "equal" } }),
      row("", 1395, {
        byKeymode: [
          { bucket: "k7", n: 1376 },
          { bucket: "unknown", n: 19 },
        ],
      }),
      row("W", 344),
      row("Rosalind", 68),
      row("s", 62),
      row("w", 33),
      row("Kovacs", 30),
      row("Wulf", 10),
      row("Sterling", 1, { nOnline: 1, nOffline: 0 }),
    ],
    ...overrides,
  };
}

/** Same aliases, but the cfg login matched none of them (R5 "no match"). */
export function noMatchAliasList(): AliasListDto {
  const list = pilotAliasList();
  return {
    ...list,
    aliases: list.aliases.map((a) => ({ ...a, autoMatch: null, selected: false, inSelfProfile: false })),
  };
}

export const SELF_PROFILE: ProfileEntryDto = {
  ref: { kind: "profile", id: 1 },
  profileKind: "self",
  label: "Me",
  isDefault: true,
  mergeMode: "merged",
  aliasIds: [1, 2],
  scopes: [{ scopeHash: "a".repeat(64), aliasIds: [1, 2], keymode: 7 }],
};

export const OTHER_PROFILE: ProfileEntryDto = {
  ref: { kind: "profile", id: 2 },
  profileKind: "other",
  label: "Rosalind",
  isDefault: false,
  mergeMode: "merged",
  aliasIds: [5],
  scopes: [{ scopeHash: "b".repeat(64), aliasIds: [5], keymode: 7 }],
};

export const ALL_PLAYERS: ProfileEntryDto = {
  ref: { kind: "all_players" },
  profileKind: "all_players",
  label: "",
  isDefault: false,
  mergeMode: "merged",
  aliasIds: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
  scopes: [{ scopeHash: "c".repeat(64), aliasIds: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10], keymode: 7 }],
};
