import { useState } from 'react';
import { BOARD_BY_ID, ROLES, ROLE_BY_ID } from '../data/hardware';
import type { BoardRoleConfig, RoleAssignment, BusType } from '../data/hardware';
import { useWizard } from '../lib/wizard-context';

const BUS_OPTIONS: BusType[] = [
  'I2C', 'SPI', 'UART', 'USB', 'Wi-Fi/MQTT', 'GPIO', 'I2S', 'CSI', 'Native',
];

interface RoleAssignmentStepProps {
  onNext: () => void;
  onBack: () => void;
}

export function RoleAssignmentStep({ onNext, onBack }: RoleAssignmentStepProps) {
  const { state, update } = useWizard();

  // Build the list of all boards in this deployment
  const allBoardIds = [
    ...(state.hostBoard ? [state.hostBoard] : []),
    ...state.peripheralBoards,
  ];

  // Local draft state — initialise from existing roleConfigs
  const [configs, setConfigs] = useState<BoardRoleConfig[]>(() => {
    return allBoardIds.map(boardId => {
      const existing = state.roleConfigs.find(rc => rc.boardId === boardId);
      return existing ?? { boardId, assignments: [] };
    });
  });

  // Track which board panel is expanded
  const [expandedBoard, setExpandedBoard] = useState<string | null>(allBoardIds[0] ?? null);

  function getConfigForBoard(boardId: string): BoardRoleConfig {
    return configs.find(c => c.boardId === boardId) ?? { boardId, assignments: [] };
  }

  function updateConfig(boardId: string, updater: (prev: BoardRoleConfig) => BoardRoleConfig) {
    setConfigs(prev =>
      prev.map(c => (c.boardId === boardId ? updater(c) : c))
    );
  }

  function toggleRole(boardId: string, roleId: string) {
    updateConfig(boardId, prev => {
      const exists = prev.assignments.some(a => a.roleId === roleId);
      if (exists) {
        return { ...prev, assignments: prev.assignments.filter(a => a.roleId !== roleId) };
      }
      const roleDef = ROLE_BY_ID[roleId];
      const newAssignment: RoleAssignment = {
        roleId,
        bus: roleDef?.defaultBus ?? 'I2C',
        pinOrAddress: '',
        notes: '',
      };
      return { ...prev, assignments: [...prev.assignments, newAssignment] };
    });
  }

  function updateAssignment(
    boardId: string,
    roleId: string,
    field: keyof RoleAssignment,
    value: string
  ) {
    updateConfig(boardId, prev => ({
      ...prev,
      assignments: prev.assignments.map(a =>
        a.roleId === roleId ? { ...a, [field]: value } : a
      ),
    }));
  }

  function handleNext() {
    update({ roleConfigs: configs.filter(c => c.assignments.length > 0) });
    onNext();
  }

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h2 className="text-xl font-bold text-white mb-1">Assign Roles to Components</h2>
        <p className="text-slate-400 text-sm">
          Each component in your deployment can have one or more roles. Select the roles for each
          board, then specify how it connects (bus type) and any relevant pin or address. Suggested
          roles for each board are highlighted in blue.
        </p>
      </div>

      {allBoardIds.map(boardId => {
        const board = BOARD_BY_ID[boardId];
        if (!board) return null;
        const config = getConfigForBoard(boardId);
        const isExpanded = expandedBoard === boardId;
        const assignedCount = config.assignments.length;

        return (
          <div
            key={boardId}
            className="rounded-xl border border-slate-700 bg-slate-800/60 overflow-hidden"
          >
            {/* Board header */}
            <button
              className="w-full flex items-center justify-between px-4 py-3 hover:bg-slate-700/40 transition-colors"
              onClick={() => setExpandedBoard(isExpanded ? null : boardId)}
            >
              <div className="flex items-center gap-3">
                <span className="text-lg">
                  {board.category === 'host' ? '🖥️' :
                   board.category === 'esp32' ? '📟' :
                   board.category === 'rpi' ? '🍓' :
                   board.category === 'arduino' ? '⚡' :
                   board.category === 'stm32' ? '🔬' : '🔧'}
                </span>
                <div className="text-left">
                  <div className="text-white font-medium text-sm">{board.displayName}</div>
                  <div className="text-slate-400 text-xs">{board.architecture}</div>
                </div>
              </div>
              <div className="flex items-center gap-3">
                {assignedCount > 0 && (
                  <span className="bg-blue-600 text-white text-xs font-medium px-2 py-0.5 rounded-full">
                    {assignedCount} role{assignedCount !== 1 ? 's' : ''}
                  </span>
                )}
                <span className="text-slate-400 text-sm">{isExpanded ? '▲' : '▼'}</span>
              </div>
            </button>

            {isExpanded && (
              <div className="px-4 pb-4 border-t border-slate-700 pt-4 flex flex-col gap-4">
                {/* Role selection grid */}
                <div>
                  <p className="text-slate-400 text-xs mb-2 uppercase tracking-wide font-medium">
                    Select roles (tap to toggle, multiple allowed)
                  </p>
                  <div className="grid grid-cols-2 sm:grid-cols-3 gap-2">
                    {ROLES.map(role => {
                      const isAssigned = config.assignments.some(a => a.roleId === role.id);
                      const isSuggested = board.suggestedRoles?.includes(role.id) ?? false;
                      const isCapable = role.requiredCapabilities.length === 0 ||
                        role.requiredCapabilities.every(cap => board.capabilities.includes(cap));

                      return (
                        <button
                          key={role.id}
                          onClick={() => toggleRole(boardId, role.id)}
                          className={`
                            flex items-center gap-2 px-3 py-2 rounded-lg border text-left text-xs transition-all
                            ${isAssigned
                              ? 'border-blue-500 bg-blue-600/20 text-blue-300'
                              : isSuggested
                              ? 'border-blue-700 bg-blue-900/20 text-slate-300 hover:border-blue-500'
                              : isCapable
                              ? 'border-slate-600 bg-slate-700/40 text-slate-400 hover:border-slate-500'
                              : 'border-slate-700 bg-slate-800/40 text-slate-600 cursor-not-allowed opacity-50'
                            }
                          `}
                          title={
                            !isCapable
                              ? `${board.displayName} lacks required capabilities for this role`
                              : isSuggested
                              ? 'Suggested for this board'
                              : ''
                          }
                        >
                          <span className="text-base">{role.icon}</span>
                          <div>
                            <div className="font-medium leading-tight">{role.label}</div>
                            {isSuggested && !isAssigned && (
                              <div className="text-blue-400 text-[10px]">suggested</div>
                            )}
                          </div>
                          {isAssigned && (
                            <span className="ml-auto text-blue-400 font-bold">✓</span>
                          )}
                        </button>
                      );
                    })}
                  </div>
                </div>

                {/* Per-role connection details */}
                {config.assignments.length > 0 && (
                  <div className="flex flex-col gap-3">
                    <p className="text-slate-400 text-xs uppercase tracking-wide font-medium">
                      Connection details
                    </p>
                    {config.assignments.map(assignment => {
                      const roleDef = ROLE_BY_ID[assignment.roleId];
                      if (!roleDef) return null;
                      return (
                        <div
                          key={assignment.roleId}
                          className="rounded-lg border border-slate-600 bg-slate-700/40 p-3 flex flex-col gap-2"
                        >
                          <div className="flex items-center gap-2 mb-1">
                            <span>{roleDef.icon}</span>
                            <span className="text-white text-sm font-medium">{roleDef.label}</span>
                          </div>

                          {/* Bus type */}
                          <div className="flex flex-col gap-1">
                            <label className="text-slate-400 text-xs">Connection Bus</label>
                            <select
                              value={assignment.bus}
                              onChange={e =>
                                updateAssignment(boardId, assignment.roleId, 'bus', e.target.value)
                              }
                              className="bg-slate-800 border border-slate-600 rounded-lg px-3 py-1.5 text-white text-sm focus:outline-none focus:border-blue-500"
                            >
                              {BUS_OPTIONS.map(bus => (
                                <option key={bus} value={bus}>{bus}</option>
                              ))}
                            </select>
                          </div>

                          {/* Pin / Address */}
                          {roleDef.hasPinField && (
                            <div className="flex flex-col gap-1">
                              <label className="text-slate-400 text-xs">{roleDef.pinLabel}</label>
                              <input
                                type="text"
                                value={assignment.pinOrAddress}
                                onChange={e =>
                                  updateAssignment(boardId, assignment.roleId, 'pinOrAddress', e.target.value)
                                }
                                placeholder={roleDef.pinPlaceholder}
                                className="bg-slate-800 border border-slate-600 rounded-lg px-3 py-1.5 text-white text-sm placeholder-slate-500 focus:outline-none focus:border-blue-500"
                              />
                            </div>
                          )}

                          {/* Notes */}
                          <div className="flex flex-col gap-1">
                            <label className="text-slate-400 text-xs">Notes (optional)</label>
                            <input
                              type="text"
                              value={assignment.notes}
                              onChange={e =>
                                updateAssignment(boardId, assignment.roleId, 'notes', e.target.value)
                              }
                              placeholder="Any extra wiring notes..."
                              className="bg-slate-800 border border-slate-600 rounded-lg px-3 py-1.5 text-white text-sm placeholder-slate-500 focus:outline-none focus:border-blue-500"
                            />
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}

                {config.assignments.length === 0 && (
                  <p className="text-slate-500 text-xs italic">
                    No roles assigned yet. Select at least one role above, or click Continue to skip
                    role assignment for this board.
                  </p>
                )}
              </div>
            )}
          </div>
        );
      })}

      {/* Navigation */}
      <div className="flex gap-3 pt-2">
        <button
          onClick={onBack}
          className="flex-1 py-2.5 rounded-xl border border-slate-600 text-slate-300 text-sm font-medium hover:border-slate-500 hover:text-white transition-colors"
        >
          ← Back
        </button>
        <button
          onClick={handleNext}
          className="flex-1 py-2.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-white text-sm font-semibold transition-colors"
        >
          Continue →
        </button>
      </div>
    </div>
  );
}
