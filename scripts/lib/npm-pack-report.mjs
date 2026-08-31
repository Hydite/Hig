export function parseNpmPackReport(output) {
  const parsed = JSON.parse(output);
  if (Array.isArray(parsed) && parsed.length === 1) return parsed[0];
  if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
    const reports = Object.values(parsed);
    if (reports.length === 1) return reports[0];
  }
  throw new Error("npm pack returned an unexpected report");
}
