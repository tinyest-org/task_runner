import { onCleanup, createEffect, on } from 'solid-js';
import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import type { DagResponse, BasicTask } from '../types';
import { STATUS_COLORS } from '../constants';
import { computeGroupLayout } from '../lib/isometricLayout';
import type { LayoutResult } from '../lib/isometricLayout';

interface Props {
  data: DagResponse | null;
  onNodeClick: (task: BasicTask) => void;
  onBackgroundClick: () => void;
}

// Layout constants
const TASK_BOX_SIZE = 0.8;
const TASK_SPACING = 1.1;
const GROUP_PADDING = 0.5;

// Reusable materials / geometries
const taskGeometry = new THREE.BoxGeometry(TASK_BOX_SIZE, TASK_BOX_SIZE, TASK_BOX_SIZE);

function colorToHex(color: string): number {
  return parseInt(color.replace('#', ''), 16);
}

function createTextSprite(text: string, fontSize: number = 48): THREE.Sprite {
  const canvas = document.createElement('canvas');
  const ctx = canvas.getContext('2d')!;
  canvas.width = 512;
  canvas.height = 128;
  ctx.fillStyle = 'rgba(255, 255, 255, 0.8)';
  ctx.font = `${fontSize}px monospace`;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(text, 256, 64);

  const texture = new THREE.CanvasTexture(canvas);
  texture.needsUpdate = true;
  const material = new THREE.SpriteMaterial({ map: texture, transparent: true });
  const sprite = new THREE.Sprite(material);
  sprite.scale.set(4, 1, 1);
  sprite.userData.isLabel = true;
  return sprite;
}

export default function IsometricView(props: Props) {
  let containerEl!: HTMLDivElement;
  let renderer: THREE.WebGLRenderer | null = null;
  let scene: THREE.Scene | null = null;
  let camera: THREE.OrthographicCamera | null = null;
  let controls: OrbitControls | null = null;
  let animFrameId: number | null = null;
  let tooltipEl: HTMLDivElement | null = null;

  // Map meshes to tasks for raycasting
  const meshToTask = new Map<THREE.Object3D, BasicTask>();
  const taskToMesh = new Map<string, THREE.Mesh>();
  let currentNodeIds = new Set<string>();
  let currentEdgeIds = new Set<string>();
  let currentStatuses = new Map<string, string>();
  let hoveredMesh: THREE.Mesh | null = null;

  const raycaster = new THREE.Raycaster();
  const pointer = new THREE.Vector2();

  function initScene() {
    scene = new THREE.Scene();
    scene.background = new THREE.Color(0x0a0a1a);

    // Camera
    const aspect = containerEl.clientWidth / containerEl.clientHeight;
    const frustum = 20;
    camera = new THREE.OrthographicCamera(
      -frustum * aspect,
      frustum * aspect,
      frustum,
      -frustum,
      0.1,
      1000,
    );
    camera.position.set(-95, 30, 40);
    camera.lookAt(0, 0, 0);

    // Renderer
    renderer = new THREE.WebGLRenderer({ antialias: true });
    renderer.setSize(containerEl.clientWidth, containerEl.clientHeight);
    renderer.setPixelRatio(window.devicePixelRatio);
    containerEl.appendChild(renderer.domElement);

    // Controls
    controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.1;

    // Lighting
    const ambient = new THREE.AmbientLight(0xffffff, 0.6);
    scene.add(ambient);
    const directional = new THREE.DirectionalLight(0xffffff, 0.8);
    directional.position.set(10, 20, 10);
    scene.add(directional);

    // Grid
    const grid = new THREE.GridHelper(50, 50, 0x333344, 0x222233);
    scene.add(grid);

    // Tooltip
    tooltipEl = document.createElement('div');
    tooltipEl.style.cssText =
      'position:fixed;pointer-events:none;background:rgba(0,0,0,0.85);color:#fff;padding:6px 10px;border-radius:4px;font-size:12px;font-family:monospace;display:none;z-index:1000;white-space:nowrap;';
    containerEl.appendChild(tooltipEl);

    // Event listeners
    renderer.domElement.addEventListener('pointermove', onPointerMove);
    renderer.domElement.addEventListener('click', onClick);
    window.addEventListener('resize', onResize);

    // Animation loop
    function animate() {
      animFrameId = requestAnimationFrame(animate);
      controls!.update();
      renderer!.render(scene!, camera!);
    }
    animate();
  }

  function onResize() {
    if (!renderer || !camera) return;
    const w = containerEl.clientWidth;
    const h = containerEl.clientHeight;
    const aspect = w / h;
    const frustum = camera.top;
    camera.left = -frustum * aspect;
    camera.right = frustum * aspect;
    camera.updateProjectionMatrix();
    renderer.setSize(w, h);
  }

  function onPointerMove(event: PointerEvent) {
    if (!renderer || !camera || !scene) return;
    const rect = renderer.domElement.getBoundingClientRect();
    pointer.x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
    pointer.y = -((event.clientY - rect.top) / rect.height) * 2 + 1;

    raycaster.setFromCamera(pointer, camera);
    const taskMeshes = Array.from(meshToTask.keys()) as THREE.Mesh[];
    const intersects = raycaster.intersectObjects(taskMeshes);

    // Reset previous hover
    if (hoveredMesh) {
      (hoveredMesh.material as THREE.MeshLambertMaterial).emissive.setHex(0x000000);
      hoveredMesh = null;
    }
    if (tooltipEl) tooltipEl.style.display = 'none';

    if (intersects.length > 0) {
      const mesh = intersects[0].object as THREE.Mesh;
      const task = meshToTask.get(mesh);
      if (task) {
        hoveredMesh = mesh;
        (mesh.material as THREE.MeshLambertMaterial).emissive.setHex(0x222222);
        renderer.domElement.style.cursor = 'pointer';

        if (tooltipEl) {
          tooltipEl.textContent = `${task.name} [${task.status}] (${task.kind})`;
          tooltipEl.style.display = 'block';
          tooltipEl.style.left = `${event.clientX + 12}px`;
          tooltipEl.style.top = `${event.clientY + 12}px`;
        }
      }
    } else {
      renderer.domElement.style.cursor = 'grab';
    }
  }

  function onClick(event: MouseEvent) {
    if (!renderer || !camera || !scene) return;
    const rect = renderer.domElement.getBoundingClientRect();
    pointer.x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
    pointer.y = -((event.clientY - rect.top) / rect.height) * 2 + 1;

    raycaster.setFromCamera(pointer, camera);
    const taskMeshes = Array.from(meshToTask.keys()) as THREE.Mesh[];
    const intersects = raycaster.intersectObjects(taskMeshes);

    if (intersects.length > 0) {
      const task = meshToTask.get(intersects[0].object);
      if (task) props.onNodeClick(task);
    } else {
      props.onBackgroundClick();
    }
  }

  function clearScene() {
    if (!scene) return;
    const toRemove: THREE.Object3D[] = [];
    scene.traverse((obj) => {
      if (obj.userData.isTask || obj.userData.isGroup || obj.userData.isLink || obj.userData.isLabel) {
        toRemove.push(obj);
      }
    });
    for (const obj of toRemove) {
      if (obj instanceof THREE.Mesh) {
        obj.geometry.dispose();
        if (Array.isArray(obj.material)) {
          obj.material.forEach((m) => m.dispose());
        } else {
          obj.material.dispose();
        }
      } else if (obj instanceof THREE.Line) {
        obj.geometry.dispose();
        if (Array.isArray(obj.material)) {
          obj.material.forEach((m) => m.dispose());
        } else {
          obj.material.dispose();
        }
      } else if (obj instanceof THREE.Sprite) {
        (obj.material as THREE.SpriteMaterial).map?.dispose();
        obj.material.dispose();
      }
      scene!.remove(obj);
    }
    meshToTask.clear();
    taskToMesh.clear();
  }

  function fitCamera(kindCount: number, maxStep: number, cellSizeX: number, cellSizeZ: number) {
    if (!camera) return;
    const extentX = kindCount * cellSizeX;
    const extentZ = (maxStep + 1) * cellSizeZ;
    const extent = Math.max(extentX, extentZ, 10) * 0.7;
    const aspect = containerEl.clientWidth / containerEl.clientHeight;
    camera.top = extent;
    camera.bottom = -extent;
    camera.left = -extent * aspect;
    camera.right = extent * aspect;
    camera.updateProjectionMatrix();
  }

  function buildScene(data: DagResponse) {
    if (!scene) return;
    clearScene();

    const layout = computeGroupLayout(data.tasks, data.links);
    const { groups, kinds, maxStep, depthMap, maxCols, maxRows, maxLayers } = layout;

    // Dynamic cell sizes based on largest group 3D grid
    const cellSizeX = Math.max(maxCols * TASK_SPACING + GROUP_PADDING * 2, 3);
    const cellSizeZ = Math.max(maxRows * TASK_SPACING + GROUP_PADDING * 2, 3);

    // Group groups by step and assign centered local X indices
    const groupsByStep = new Map<number, GroupLayout[]>();
    for (const group of groups) {
      const list = groupsByStep.get(group.stepIndex) || [];
      list.push(group);
      groupsByStep.set(group.stepIndex, list);
    }
    const groupCx = new Map<GroupLayout, number>();
    for (const [, stepGroups] of groupsByStep) {
      stepGroups.sort((a, b) => a.kindIndex - b.kindIndex);
      const count = stepGroups.length;
      for (let i = 0; i < count; i++) {
        groupCx.set(stepGroups[i], (i - (count - 1) / 2) * cellSizeX);
      }
    }

    // Center offset Z
    const offsetZ = (maxStep * cellSizeZ) / 2;

    // Task position map for links
    const taskPositions = new Map<string, THREE.Vector3>();

    // Render groups and tasks
    for (const group of groups) {
      const cx = groupCx.get(group)!;
      const cz = group.stepIndex * cellSizeZ - offsetZ;
      const sliceSize = group.cols * group.rows;

      // Uniform wireframe volume (same size for all groups)
      const groupWidth = maxCols * TASK_SPACING + GROUP_PADDING;
      const groupDepth = maxRows * TASK_SPACING + GROUP_PADDING;
      const groupHeight = maxLayers * TASK_SPACING + GROUP_PADDING;
      const boxGeo = new THREE.BoxGeometry(groupWidth, groupHeight, groupDepth);
      const edges = new THREE.EdgesGeometry(boxGeo);
      const lineMat = new THREE.LineBasicMaterial({ color: 0x444466, transparent: true, opacity: 0.5 });
      const wireframe = new THREE.LineSegments(edges, lineMat);
      wireframe.position.set(cx, groupHeight / 2, cz);
      wireframe.userData.isGroup = true;
      scene.add(wireframe);
      boxGeo.dispose();

      // Individual task cubes placed in 3D XYZ grid
      for (let i = 0; i < group.tasks.length; i++) {
        const task = group.tasks[i];
        const layer = Math.floor(i / sliceSize);
        const inSlice = i % sliceSize;
        const col = inSlice % group.cols;
        const row = Math.floor(inSlice / group.cols);

        const color = STATUS_COLORS[task.status] ?? '#666666';
        const material = new THREE.MeshLambertMaterial({ color: colorToHex(color) });
        const mesh = new THREE.Mesh(taskGeometry, material);

        const localX = (col - (group.cols - 1) / 2) * TASK_SPACING;
        const localZ = (row - (group.rows - 1) / 2) * TASK_SPACING;
        const py = TASK_BOX_SIZE / 2 + layer * TASK_SPACING;
        mesh.position.set(cx + localX, py, cz + localZ);
        mesh.userData.isTask = true;
        scene.add(mesh);

        meshToTask.set(mesh, task);
        taskToMesh.set(task.id, mesh);
        taskPositions.set(task.id, mesh.position.clone());
      }
    }

    // Dependency links
    for (const link of data.links) {
      const from = taskPositions.get(link.parent_id);
      const to = taskPositions.get(link.child_id);
      if (!from || !to) continue;

      const points = [from, to];
      const geometry = new THREE.BufferGeometry().setFromPoints(points);
      const color = link.requires_success ? 0x27ae60 : 0x666666;
      const material = new THREE.LineBasicMaterial({
        color,
        transparent: true,
        opacity: 0.6,
      });
      const line = new THREE.Line(geometry, material);
      line.userData.isLink = true;
      scene.add(line);
    }

    // Kind labels placed at each group's position
    for (const group of groups) {
      const cx = groupCx.get(group)!;
      const cz = group.stepIndex * cellSizeZ - offsetZ;
      const sprite = createTextSprite(group.key.kind, 40);
      sprite.position.set(cx, -1.5, cz - cellSizeZ / 2 - 1);
      scene.add(sprite);
    }

    // Step labels (along Z axis) — hidden for now
    // const allCxValues = Array.from(groupCx.values());
    // const minCx = Math.min(...allCxValues);
    // for (let s = 0; s <= maxStep; s++) {
    //   const sprite = createTextSprite(`step ${s}`, 36);
    //   sprite.position.set(minCx - 4, -1.5, s * cellSizeZ - offsetZ);
    //   scene.add(sprite);
    // }

    fitCamera(kinds.length, maxStep, cellSizeX, cellSizeZ);

    currentNodeIds = new Set(data.tasks.map((t) => t.id));
    currentEdgeIds = new Set(data.links.map((l) => `${l.parent_id}-${l.child_id}`));
    currentStatuses = new Map(data.tasks.map((t) => [t.id, t.status]));
  }

  function updateColorsInPlace(tasks: BasicTask[]) {
    for (const task of tasks) {
      const mesh = taskToMesh.get(task.id);
      if (!mesh) continue;
      const color = STATUS_COLORS[task.status] ?? '#666666';
      (mesh.material as THREE.MeshLambertMaterial).color.setHex(colorToHex(color));
      // Update task reference in meshToTask
      meshToTask.set(mesh, task);
    }
  }

  function structureChanged(tasks: BasicTask[], links: DagResponse['links']): boolean {
    if (tasks.length !== currentNodeIds.size) return true;
    for (const t of tasks) {
      if (!currentNodeIds.has(t.id)) return true;
      if (currentStatuses.get(t.id) !== t.status) return true;
    }
    const newEdgeIds = new Set(links.map((l) => `${l.parent_id}-${l.child_id}`));
    if (newEdgeIds.size !== currentEdgeIds.size) return true;
    for (const eid of newEdgeIds) {
      if (!currentEdgeIds.has(eid)) return true;
    }
    return false;
  }

  createEffect(
    on(
      () => props.data,
      (data) => {
        if (!data || data.tasks.length === 0) {
          clearScene();
          currentNodeIds.clear();
          currentEdgeIds.clear();
          currentStatuses.clear();
          return;
        }

        if (!renderer) {
          initScene();
        }

        if (currentNodeIds.size > 0 && !structureChanged(data.tasks, data.links)) {
          updateColorsInPlace(data.tasks);
        } else {
          buildScene(data);
        }
      },
    ),
  );

  onCleanup(() => {
    if (animFrameId !== null) cancelAnimationFrame(animFrameId);
    if (controls) controls.dispose();
    if (renderer) {
      renderer.domElement.removeEventListener('pointermove', onPointerMove);
      renderer.domElement.removeEventListener('click', onClick);
      renderer.dispose();
      if (renderer.domElement.parentNode) {
        renderer.domElement.parentNode.removeChild(renderer.domElement);
      }
    }
    if (tooltipEl && tooltipEl.parentNode) {
      tooltipEl.parentNode.removeChild(tooltipEl);
    }
    window.removeEventListener('resize', onResize);
    clearScene();
    scene = null;
    camera = null;
    renderer = null;
    controls = null;
  });

  return (
    <div
      ref={containerEl}
      class="flex-1"
      style={{ background: 'transparent' }}
    />
  );
}
