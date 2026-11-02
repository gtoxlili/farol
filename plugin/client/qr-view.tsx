// Draws QR modules with plain Views so it renders the same on web, iOS and
// Android. Dark runs in a row merge into one View. The code keeps dark-on-white
// with a quiet zone in every theme: that is what cameras read.
import { useMemo } from "react";
import { View } from "react-native";

const QUIET = 4;
const DARK = "#000000";
const LIGHT = "#ffffff";

interface Run {
  start: number;
  length: number;
}

function runs(row: string): Run[] {
  const out: Run[] = [];
  let i = 0;
  while (i < row.length) {
    if (row[i] !== "1") {
      i++;
      continue;
    }
    const start = i;
    while (i < row.length && row[i] === "1") i++;
    out.push({ start, length: i - start });
  }
  return out;
}

export function QrView({ rows, size }: { rows: readonly string[]; size: number }) {
  const n = rows.length;
  const cell = Math.max(1, Math.floor(size / (n + QUIET * 2)));
  const side = cell * (n + QUIET * 2);
  const lines = useMemo(() => rows.map(runs), [rows]);
  return (
    <View
      accessibilityRole="image"
      accessibilityLabel="Farol 连接码"
      style={{ width: side, height: side, backgroundColor: LIGHT, padding: cell * QUIET }}
    >
      {lines.map((line, r) => (
        <View key={r} style={{ height: cell, width: cell * n, flexDirection: "row" }}>
          {line.map((run) => (
            <View
              key={run.start}
              style={{
                position: "absolute",
                left: run.start * cell,
                width: run.length * cell,
                height: cell,
                backgroundColor: DARK,
              }}
            />
          ))}
        </View>
      ))}
    </View>
  );
}
