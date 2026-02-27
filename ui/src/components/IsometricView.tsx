import { onCleanup, createEffect, on } from 'solid-js';
import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import type { DagResponse, BasicTask } from '../types';
import { STATUS_COLORS } from '../constants';
import type { CriticalPath } from '../lib/criticalPath';
import { formatDuration } from '../lib/format';
import { structureChanged } from '../lib/dagDiff';
import { colorToHex, disposeObject } from '../lib/threeHelpers';
import {
  buildDagSceneObjects,
  buildStatusSceneObjects,
  computeStatusAnimationTargets,
  applyCriticalPathHighlighting,
} from '../lib/isometricSceneBuilder';

interface Props {
  data: DagResponse | null;
  criticalPath?: CriticalPath | null;
  onNodeClick: (task: BasicTask) => void;
  onBackgroundClick: () => void;
  groupBy?: 'dag' | 'status';
  onResetCamera?: (fn: () => void) => void;
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

  // Animation state for status-mode transitions
  const animTargets = new Map<string, THREE.Vector3>();
  let isAnimating = false;
  let currentGroupBy: 'dag' | 'status' = 'dag';

  const raycaster = new THREE.Raycaster();
  const pointer = new THREE.Vector2();

  const refs = { meshToTask, taskToMesh };

  function initScene() {
    scene = new THREE.Scene();
    scene.background = new THREE.Color(0x0a0a1a);

    const aspect = containerEl.clientWidth / containerEl.clientHeight;
    const frustum = 20;
    camera = new THREE.OrthographicCamera(
      -frustum * aspect, frustum * aspect, frustum, -frustum, 0.1, 1000,
    );
    camera.position.set(-95, 30, 40);
    camera.lookAt(0, 0, 0);

    renderer = new THREE.WebGLRenderer({ antialias: true });
    renderer.setSize(containerEl.clientWidth, containerEl.clientHeight);
    renderer.setPixelRatio(window.devicePixelRatio);
    containerEl.appendChild(renderer.domElement);

    controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.1;

    const ambient = new THREE.AmbientLight(0xffffff, 0.6);
    scene.add(ambient);
    const directional = new THREE.DirectionalLight(0xffffff, 0.8);
    directional.position.set(10, 20, 10);
    scene.add(directional);

    const grid = new THREE.GridHelper(50, 50, 0x333344, 0x222233);
    scene.add(grid);

    // Tooltip
    tooltipEl = document.createElement('div');
    tooltipEl.style.cssText =
      'position:fixed;pointer-events:none;background:rgba(0,0,0,0.9);color:#fff;padding:8px 12px;border-radius:6px;font-size:12px;font-family:monospace;display:none;z-index:1000;white-space:pre;line-height:1.5;border:1px solid rgba(255,255,255,0.1);';
    containerEl.appendChild(tooltipEl);

    renderer.domElement.addEventListener('pointermove', onPointerMove);
    renderer.domElement.addEventListener('click', onClick);
    window.addEventListener('resize', onResize);

    function animate() {
      animFrameId = requestAnimationFrame(animate);

      if (isAnimating) {
        let allDone = true;
        for (const [taskId, target] of animTargets) {
          const mesh = taskToMesh.get(taskId);
          if (!mesh) continue;
          if (mesh.position.distanceTo(target) > 0.01) {
            mesh.position.lerp(target, 0.08);
            allDone = false;
          } else {
            mesh.position.copy(target);
          }
        }
        if (allDone) {
          isAnimating = false;
          animTargets.clear();
        }
      }

      controls!.update();
      renderer!.render(scene!, camera!);
    }
    animate();

    props.onResetCamera?.(() => {
      if (!camera || !controls) return;
      camera.position.set(-95, 30, 40);
      camera.lookAt(0, 0, 0);
      controls.target.set(0, 0, 0);
      controls.update();
    });
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

    if (hoveredMesh) {
      const prevTask = meshToTask.get(hoveredMesh);
      const isCritical = prevTask && props.criticalPath?.nodeIds.has(prevTask.id);
      (hoveredMesh.material as THREE.MeshLambertMaterial).emissive.setHex(
        isCritical ? 0x665500 : 0x000000,
      );
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
          const lines = [`${task.name}`, `${task.status} | ${task.kind}`];
          if (task.success || task.failures) {
            let counters = `\u2713 ${task.success}  \u2717 ${task.failures}`;
            if (task.expected_count) {
              const pct = Math.min(
                100,
                Math.round(((task.success + task.failures) / task.expected_count) * 100),
              );
              counters += `  (${pct}%)`;
            }
            lines.push(counters);
          }
          if (task.started_at) {
            const start = new Date(task.started_at).getTime();
            const end = task.ended_at ? new Date(task.ended_at).getTime() : Date.now();
            lines.push(`\u23f1 ${formatDuration(end - start)}`);
          }
          tooltipEl.textContent = lines.join('\n');
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
    isAnimating = false;
    animTargets.clear();
    const toRemove: THREE.Object3D[] = [];
    scene.traverse((obj) => {
      if (obj.userData.isTask || obj.userData.isGroup || obj.userData.isLink || obj.userData.isLabel) {
        toRemove.push(obj);
      }
    });
    for (const obj of toRemove) {
      disposeObject(obj);
      scene!.remove(obj);
    }
    meshToTask.clear();
    taskToMesh.clear();
  }

  function clearStructuralElements() {
    if (!scene) return;
    const toRemove: THREE.Object3D[] = [];
    scene.traverse((obj) => {
      if (obj.userData.isGroup || obj.userData.isLink || obj.userData.isLabel) {
        toRemove.push(obj);
      }
    });
    for (const obj of toRemove) {
      disposeObject(obj);
      scene!.remove(obj);
    }
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

  function fitCameraExtent(extentX: number, extentZ: number) {
    if (!camera) return;
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
    const result = buildDagSceneObjects(scene, data, refs);
    fitCamera(result.kindCount, result.maxStep, result.cellSizeX, result.cellSizeZ);
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
      meshToTask.set(mesh, task);
    }
    currentStatuses = new Map(tasks.map((t) => [t.id, t.status]));
  }

  function taskSetChanged(tasks: BasicTask[]): boolean {
    if (tasks.length !== currentNodeIds.size) return true;
    for (const t of tasks) {
      if (!currentNodeIds.has(t.id)) return true;
    }
    return false;
  }

  function buildStatusScene(data: DagResponse) {
    if (!scene) return;
    clearScene();
    const result = buildStatusSceneObjects(scene, data, refs);
    fitCameraExtent(result.extentX, result.extentZ);
    currentNodeIds = new Set(data.tasks.map((t) => t.id));
    currentEdgeIds = new Set(data.links.map((l) => `${l.parent_id}-${l.child_id}`));
    currentStatuses = new Map(data.tasks.map((t) => [t.id, t.status]));
  }

  function animateToStatusLayout(data: DagResponse) {
    if (!scene) return;

    // Update colors on existing meshes
    for (const task of data.tasks) {
      const mesh = taskToMesh.get(task.id);
      if (mesh) {
        const color = STATUS_COLORS[task.status] ?? '#666666';
        (mesh.material as THREE.MeshLambertMaterial).color.setHex(colorToHex(color));
        meshToTask.set(mesh, task);
      }
    }

    clearStructuralElements();
    const result = computeStatusAnimationTargets(scene, data, refs);

    animTargets.clear();
    for (const [taskId, pos] of result.targets) {
      animTargets.set(taskId, pos);
    }
    isAnimating = true;

    fitCameraExtent(result.extentX, result.extentZ);
    currentStatuses = new Map(data.tasks.map((t) => [t.id, t.status]));
  }

  createEffect(
    on(
      () => props.criticalPath,
      (cp) => {
        if (scene) applyCriticalPathHighlighting(scene, taskToMesh, cp);
      },
    ),
  );

  createEffect(
    on(
      [() => props.data, () => props.groupBy ?? 'dag'] as const,
      ([data, groupBy]) => {
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

        const groupByChanged = groupBy !== currentGroupBy;
        currentGroupBy = groupBy;

        if (groupBy === 'status') {
          if (groupByChanged || currentNodeIds.size === 0 || taskSetChanged(data.tasks)) {
            buildStatusScene(data);
          } else {
            animateToStatusLayout(data);
          }
        } else {
          if (!groupByChanged && currentNodeIds.size > 0 && !structureChanged(data.tasks, data.links, currentNodeIds, currentEdgeIds)) {
            updateColorsInPlace(data.tasks);
          } else {
            buildScene(data);
          }
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
    isAnimating = false;
    animTargets.clear();
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
