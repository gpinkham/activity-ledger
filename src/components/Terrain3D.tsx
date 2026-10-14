import { useMemo } from "react";
import { Canvas } from "@react-three/fiber";
import { OrbitControls, Html } from "@react-three/drei";
import { EffectComposer, Bloom } from "@react-three/postprocessing";
import * as THREE from "three";
import type { HeatCell } from "../lib/types";

const COLS = 24; // hours
const ROWS = 7; // weekdays
const WIDTH = 12;
const DEPTH = 4;
const MAX_H = 3;

const DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

// grid position helpers (must match the geometry mapping below)
const xForHour = (i: number) => -WIDTH / 2 + (i * WIDTH) / (COLS - 1); // hour → X
const zForDay = (j: number) => -DEPTH / 2 + (j * DEPTH) / (ROWS - 1); // weekday → Z

function TerrainMesh({ cells }: { cells: HeatCell[] }) {
  const geometry = useMemo(() => {
    const geo = new THREE.PlaneGeometry(WIDTH, DEPTH, COLS - 1, ROWS - 1);
    geo.rotateX(-Math.PI / 2); // lie flat, Y up
    const pos = geo.attributes.position as THREE.BufferAttribute;
    const lut = new Map(cells.map((c) => [`${c.weekday}-${c.hour}`, c.count]));
    const max = cells.reduce((m, c) => Math.max(m, c.count), 0) || 1;
    const teal = new THREE.Color("#2dd4bf");
    const mag = new THREE.Color("#e879f9");
    const colors: number[] = [];
    for (let j = 0; j < ROWS; j++) {
      for (let i = 0; i < COLS; i++) {
        const idx = j * COLS + i;
        const v = lut.get(`${j}-${i}`) ?? 0;
        const t = v / max;
        pos.setY(idx, t * MAX_H);
        const col = teal.clone().lerp(mag, t);
        colors.push(col.r, col.g, col.b);
      }
    }
    pos.needsUpdate = true;
    geo.setAttribute("color", new THREE.Float32BufferAttribute(colors, 3));
    geo.computeVertexNormals();
    return geo;
  }, [cells]);

  return (
    <group>
      <mesh geometry={geometry}>
        <meshStandardMaterial vertexColors roughness={0.5} metalness={0.25} />
      </mesh>
      <lineSegments>
        <wireframeGeometry args={[geometry]} />
        <lineBasicMaterial color="#7fffe4" transparent opacity={0.16} />
      </lineSegments>
    </group>
  );
}

// Upright, DOM-based axis labels (uses app fonts, no remote font fetch).
function tick(text: string, pos: [number, number, number], strong = false) {
  return (
    <Html
      key={`${text}-${pos[0]}-${pos[2]}`}
      position={pos}
      center
      zIndexRange={[20, 0]}
      className="pointer-events-none select-none whitespace-nowrap"
      style={{ color: strong ? "#e6e8ee" : "#aab0c0", fontSize: strong ? 11 : 10, fontWeight: strong ? 600 : 400 }}
    >
      {text}
    </Html>
  );
}

function AxisLabels() {
  const hours = [0, 3, 6, 9, 12, 15, 18, 21];
  const frontZ = DEPTH / 2 + 0.6; // hour ticks along the front edge
  const leftX = -WIDTH / 2 - 0.7; // weekday ticks along the left edge
  return (
    <group>
      {hours.map((h) => tick(String(h), [xForHour(h), 0.05, frontZ]))}
      {tick("Hour (UTC)", [0, 0.05, frontZ + 0.7], true)}
      {DAYS.map((d, j) => tick(d, [leftX, 0.05, zForDay(j)]))}
      {tick("Weekday", [leftX - 1.0, 0.05, 0], true)}
    </group>
  );
}

export default function Terrain3D({ cells }: { cells: HeatCell[] }) {
  return (
    <div className="relative h-[340px] w-full rounded-lg overflow-hidden border border-line">
      <Canvas camera={{ position: [0, 6, 10], fov: 45 }} dpr={[1, 2]}>
        <color attach="background" args={["#0a0b12"]} />
        <ambientLight intensity={0.5} />
        <directionalLight position={[6, 10, 6]} intensity={1.2} />
        <pointLight position={[-5, 6, -2]} intensity={1.4} decay={0} color="#e879f9" />
        <pointLight position={[5, 4, 4]} intensity={1.0} decay={0} color="#2dd4bf" />
        <TerrainMesh cells={cells} />
        <AxisLabels />
        <OrbitControls
          enablePan={false}
          autoRotate
          autoRotateSpeed={0.4}
          minDistance={6}
          maxDistance={18}
          minPolarAngle={0.15}
          maxPolarAngle={Math.PI / 2.05}
        />
        <EffectComposer>
          <Bloom intensity={0.7} luminanceThreshold={0.25} luminanceSmoothing={0.9} mipmapBlur />
        </EffectComposer>
      </Canvas>

      {/* height/color legend (fixed overlay, always upright) */}
      <div className="pointer-events-none absolute bottom-2 left-2 rounded-md border border-line bg-bg/70 backdrop-blur px-2 py-1.5 text-[10px] text-muted">
        <div className="mb-1 text-ink">Messages per hour</div>
        <div className="flex items-center gap-1.5">
          <span>fewer</span>
          <span
            className="h-2 w-24 rounded-full"
            style={{ background: "linear-gradient(90deg,#2dd4bf,#60a5fa,#e879f9)" }}
          />
          <span>more</span>
        </div>
        <div className="mt-1 opacity-80">Taller &amp; pinker = busier hour</div>
      </div>
    </div>
  );
}
