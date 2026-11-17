/** Shown whenever the mock backend is active so nobody mistakes sample data for the real thing. */
export function MockRibbon() {
  return (
    <div
      role="note"
      className="pointer-events-none fixed top-0 right-4 z-50 rounded-b-md bg-warn px-3 py-0.5 text-[11px] font-bold tracking-widest text-[#241a00] shadow-card"
    >
      MOCK BACKEND
    </div>
  );
}
