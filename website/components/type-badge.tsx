const STYLES: Record<string, string> = {
  agent: "bg-blue-50 text-blue-700 border-blue-200",
  "mcp-server": "bg-teal-50 text-teal-700 border-teal-200",
};

export default function TypeBadge({ type }: { type: string }) {
  const style = STYLES[type] ?? "bg-gray-50 text-gray-600 border-gray-200";
  return (
    <span
      className={`inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium ${style}`}
    >
      {type}
    </span>
  );
}
