import * as THREE from 'three';
import type { DagResponse, BasicTask } from '../types';
import { STATUS_COLORS } from '../constants';
import { computeGroupLayout, computeStatusLayout } from './isometricLayout';
import type { GroupLayout } from './isometricLayout';
import type { CriticalPath } from './criticalPath';
import {
  TASK_BOX_SIZE,
  TASK_SPACING,
  GROUP_PADDING,
  taskGeometry,
  arrowGeometry,
  colorToHex,
  createTextSprite,
} from './threeHelpers';

export interface SceneRefs {
  meshToTask: Map<THREE.Object3D, BasicTask>;
  taskToMesh: Map<string, THREE.Mesh>;
}

/** Compute cell spacing for status layout. */
function statusCellSizeX(maxCols: number): number {
  return Math.max(maxCols * TASK_SPACING + GROUP_PADDING * 2, 3);
}
function statusCellSizeZ(maxRows: number): number {
  return Math.max(maxRows * TASK_SPACING + GROUP_PADDING * 2, 3);
}

/** Build the DAG-mode scene objects: wireframe groups, task cubes, links, labels. */
export function buildDagSceneObjects(
  scene: THREE.Scene,
  data: DagResponse,
  refs: SceneRefs,
): { kindCount: number; maxStep: number; cellSizeX: number; cellSizeZ: number } {
  const layout = computeGroupLayout(data.tasks, data.links);
  const { groups, kinds, maxStep, maxCols, maxRows, maxLayers } = layout;

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

  const offsetZ = (maxStep * cellSizeZ) / 2;
  const taskPositions = new Map<string, THREE.Vector3>();

  // Render groups and tasks
  for (const group of groups) {
    const cx = groupCx.get(group)!;
    const cz = group.stepIndex * cellSizeZ - offsetZ;
    const sliceSize = group.cols * group.rows;

    // Uniform wireframe volume
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

      refs.meshToTask.set(mesh, task);
      refs.taskToMesh.set(task.id, mesh);
      taskPositions.set(task.id, mesh.position.clone());
    }
  }

  // Dependency links with arrow tips
  for (const link of data.links) {
    const from = taskPositions.get(link.parent_id);
    const to = taskPositions.get(link.child_id);
    if (!from || !to) continue;

    const edgeId = `${link.parent_id}-${link.child_id}`;
    const color = link.requires_success ? 0x27ae60 : 0x666666;

    const points = [from, to];
    const geometry = new THREE.BufferGeometry().setFromPoints(points);
    const material = new THREE.LineBasicMaterial({ color, transparent: true, opacity: 0.6 });
    const line = new THREE.Line(geometry, material);
    line.userData.isLink = true;
    line.userData.edgeId = edgeId;
    line.userData.originalColor = color;
    scene.add(line);

    // Arrow cone
    const dir = new THREE.Vector3().subVectors(to, from).normalize();
    const arrowPos = new THREE.Vector3().lerpVectors(from, to, 0.85);
    const arrowMat = new THREE.MeshLambertMaterial({ color, transparent: true, opacity: 0.8 });
    const cone = new THREE.Mesh(arrowGeometry, arrowMat);
    cone.position.copy(arrowPos);
    const up = new THREE.Vector3(0, 1, 0);
    if (dir.y < -0.999) {
      cone.rotation.set(Math.PI, 0, 0);
    } else {
      const quat = new THREE.Quaternion().setFromUnitVectors(up, dir);
      cone.setRotationFromQuaternion(quat);
    }
    cone.userData.isLink = true;
    cone.userData.edgeId = edgeId;
    cone.userData.originalColor = color;
    scene.add(cone);
  }

  // Kind labels
  for (const group of groups) {
    const cx = groupCx.get(group)!;
    const cz = group.stepIndex * cellSizeZ - offsetZ;
    const sprite = createTextSprite(group.key.kind, 40);
    sprite.position.set(cx, -1.5, cz - cellSizeZ / 2 - 1);
    scene.add(sprite);
  }

  return { kindCount: kinds.length, maxStep, cellSizeX, cellSizeZ };
}

/** Build the status-mode scene objects from scratch. */
export function buildStatusSceneObjects(
  scene: THREE.Scene,
  data: DagResponse,
  refs: SceneRefs,
): { extentX: number; extentZ: number } {
  const layout = computeStatusLayout(data.tasks, data.links);
  const { groups, statuses, maxStep, maxCols, maxRows } = layout;

  const cellX = statusCellSizeX(maxCols);
  const cellZ = statusCellSizeZ(maxRows);
  const totalWidth = statuses.length * cellX;
  const offsetX = (totalWidth - cellX) / 2;
  const offsetZ = (maxStep * cellZ) / 2;

  for (const group of groups) {
    const cx = group.statusIndex * cellX - offsetX;
    const cz = group.stepIndex * cellZ - offsetZ;
    const sliceSize = group.cols * group.rows;

    const gw = group.cols * TASK_SPACING + GROUP_PADDING;
    const gd = group.rows * TASK_SPACING + GROUP_PADDING;
    const gh = group.layers * TASK_SPACING + GROUP_PADDING;
    const boxGeo = new THREE.BoxGeometry(gw, gh, gd);
    const edges = new THREE.EdgesGeometry(boxGeo);
    const wireColor = colorToHex(STATUS_COLORS[group.status] ?? '#444466');
    const lineMat = new THREE.LineBasicMaterial({ color: wireColor, transparent: true, opacity: 0.4 });
    const wireframe = new THREE.LineSegments(edges, lineMat);
    wireframe.position.set(cx, gh / 2, cz);
    wireframe.userData.isGroup = true;
    scene.add(wireframe);
    boxGeo.dispose();

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

      refs.meshToTask.set(mesh, task);
      refs.taskToMesh.set(task.id, mesh);
    }
  }

  // Status column labels
  for (let i = 0; i < statuses.length; i++) {
    const cx = i * cellX - offsetX;
    const sprite = createTextSprite(statuses[i], 40);
    sprite.position.set(cx, -1.5, -offsetZ - cellZ / 2 - 1);
    scene.add(sprite);
  }

  // Step row labels
  for (let s = 0; s <= maxStep; s++) {
    const cz = s * cellZ - offsetZ;
    const sprite = createTextSprite(`Step ${s}`, 32);
    sprite.position.set(-offsetX - cellX / 2 - 2, -1.5, cz);
    scene.add(sprite);
  }

  return { extentX: totalWidth, extentZ: (maxStep + 1) * cellZ };
}

/** Compute animated positions + rebuild structural elements for status-mode transitions. */
export function computeStatusAnimationTargets(
  scene: THREE.Scene,
  data: DagResponse,
  refs: SceneRefs,
): { targets: Map<string, THREE.Vector3>; extentX: number; extentZ: number } {
  const layout = computeStatusLayout(data.tasks, data.links);
  const { groups, statuses, maxStep, maxCols, maxRows } = layout;

  const cellX = statusCellSizeX(maxCols);
  const cellZ = statusCellSizeZ(maxRows);
  const totalWidth = statuses.length * cellX;
  const offsetX = (totalWidth - cellX) / 2;
  const offsetZ = (maxStep * cellZ) / 2;

  const targets = new Map<string, THREE.Vector3>();
  for (const group of groups) {
    const cx = group.statusIndex * cellX - offsetX;
    const cz = group.stepIndex * cellZ - offsetZ;
    const sliceSize = group.cols * group.rows;

    for (let i = 0; i < group.tasks.length; i++) {
      const task = group.tasks[i];
      const layer = Math.floor(i / sliceSize);
      const inSlice = i % sliceSize;
      const col = inSlice % group.cols;
      const row = Math.floor(inSlice / group.cols);

      const localX = (col - (group.cols - 1) / 2) * TASK_SPACING;
      const localZ = (row - (group.rows - 1) / 2) * TASK_SPACING;
      const py = TASK_BOX_SIZE / 2 + layer * TASK_SPACING;

      targets.set(task.id, new THREE.Vector3(cx + localX, py, cz + localZ));
    }
  }

  // Rebuild wireframes for new layout
  for (const group of groups) {
    const cx = group.statusIndex * cellX - offsetX;
    const cz = group.stepIndex * cellZ - offsetZ;

    const gw = group.cols * TASK_SPACING + GROUP_PADDING;
    const gd = group.rows * TASK_SPACING + GROUP_PADDING;
    const gh = group.layers * TASK_SPACING + GROUP_PADDING;
    const boxGeo = new THREE.BoxGeometry(gw, gh, gd);
    const edges = new THREE.EdgesGeometry(boxGeo);
    const wireColor = colorToHex(STATUS_COLORS[group.status] ?? '#444466');
    const lineMat = new THREE.LineBasicMaterial({ color: wireColor, transparent: true, opacity: 0.4 });
    const wireframe = new THREE.LineSegments(edges, lineMat);
    wireframe.position.set(cx, gh / 2, cz);
    wireframe.userData.isGroup = true;
    scene.add(wireframe);
    boxGeo.dispose();
  }

  // Status column labels
  for (let i = 0; i < statuses.length; i++) {
    const cx = i * cellX - offsetX;
    const sprite = createTextSprite(statuses[i], 40);
    sprite.position.set(cx, -1.5, -offsetZ - cellZ / 2 - 1);
    scene.add(sprite);
  }

  // Step row labels
  for (let s = 0; s <= maxStep; s++) {
    const cz = s * cellZ - offsetZ;
    const sprite = createTextSprite(`Step ${s}`, 32);
    sprite.position.set(-offsetX - cellX / 2 - 2, -1.5, cz);
    scene.add(sprite);
  }

  return { targets, extentX: totalWidth, extentZ: (maxStep + 1) * cellZ };
}

/** Apply or remove critical path highlighting on nodes and edges. */
export function applyCriticalPathHighlighting(
  scene: THREE.Scene,
  taskToMesh: Map<string, THREE.Mesh>,
  cp: CriticalPath | null | undefined,
): void {
  for (const [taskId, mesh] of taskToMesh) {
    const mat = mesh.material as THREE.MeshLambertMaterial;
    if (cp && cp.nodeIds.has(taskId)) {
      mat.emissive.setHex(0x665500);
    } else {
      mat.emissive.setHex(0x000000);
    }
  }

  scene.traverse((obj) => {
    if (!obj.userData.isLink || !obj.userData.edgeId) return;
    const edgeId = obj.userData.edgeId as string;
    const isCritical = cp && cp.edgeIds.has(edgeId);
    const origColor = (obj.userData.originalColor as number) ?? 0x666666;
    if (obj instanceof THREE.Line) {
      const mat = obj.material as THREE.LineBasicMaterial;
      if (isCritical) {
        mat.color.setHex(0xffd700);
        mat.opacity = 1.0;
      } else {
        mat.color.setHex(origColor);
        mat.opacity = 0.6;
      }
    } else if (obj instanceof THREE.Mesh && obj.geometry === arrowGeometry) {
      const mat = obj.material as THREE.MeshLambertMaterial;
      if (isCritical) {
        mat.color.setHex(0xffd700);
        mat.opacity = 1.0;
      } else {
        mat.color.setHex(origColor);
        mat.opacity = 0.8;
      }
    }
  });
}
