export type PrinterKind = "U1" | "A1 mini";

export type PrinterPreference = "auto" | "u1" | "a1-mini";

export type PrintStrategy = "cmyx" | "cmyx-solid" | "direct" | "a1-mono";

export type ToolheadId = "T1" | "T2" | "T3" | "T4";

export type ColorQuality = "Excellent" | "Very Good" | "Review";

export type Confidence = "Measured" | "High" | "Nominal";

export type MaterialStatus =
  "Exact" | "Close" | "Review" | "Poor" | "Material mismatch";

/** Materials that the desktop inventory can currently validate and schedule. */
export type SpoolMaterial = "PLA" | "PETG";

export interface ProjectSelection {
  fileName: string;
  sourcePath?: string;
  browserFile?: File;
}

export interface ProjectSummary {
  fileName: string;
  sourceHash: string;
  sourceDialect: string;
  sourceApplication: string;
  sourceByteSize: number;
  sourcePlateCount: number;
  objectCount: number;
  instanceCount: number;
  partCount: number;
  printablePartCount: number;
  paintedPartCount: number;
  boundedInstanceCount: number;
  usedFilamentCount: number;
  unusedFilamentCount: number;
  alternativePlateCount: number;
}

export interface AlternativePlate {
  id: number;
  name: string;
  included: boolean;
}

export interface PhysicalSpool {
  id: string;
  /** Native measured-color identity. Rotated when lot/reference changes. */
  calibrationIdentity?: string;
  source: "built-in" | "user";
  name: string;
  colorName: string;
  hex: string;
  material: SpoolMaterial;
  sku?: string;
  profile?: string;
  vendor?: string;
  productLine?: string;
  opticalDescriptor?: string;
  minNozzleTemperatureC?: number;
  maxNozzleTemperatureC?: number;
  batchLot?: string;
  calibrationReference?: string;
  notes?: string;
  colorBasis: "Measured" | "Nominal";
  /** Out-of-stock spools stay in the catalogue but cannot be scheduled. */
  available: boolean;
}

export interface NewPhysicalSpoolInput {
  name: string;
  colorName: string;
  hex: string;
  material: SpoolMaterial;
  sku?: string;
  profile?: string;
  vendor?: string;
  productLine?: string;
  opticalDescriptor?: string;
  minNozzleTemperatureC?: number;
  maxNozzleTemperatureC?: number;
  batchLot?: string;
  calibrationReference?: string;
  notes?: string;
}

export interface LoadedToolhead {
  toolhead: ToolheadId;
  spoolId: string;
}

export interface PrinterLoadoutSnapshot {
  currentLoadout: LoadedToolhead[];
  currentA1SpoolId: string | null;
}

export interface DirectColorMapping {
  id: string;
  /** Stable cross-scope source identity used for non-destructive inheritance. */
  inheritanceKey: string;
  /** Role-separated rows sharing this identity must use one spool/toolhead. */
  physicalIdentityId: string;
  sourceSlot: string;
  sourceName: string;
  sourceHex: string;
  sourceMaterial: "PLA" | "PETG";
  usedBy: string;
  cmyRecipe: string;
  cmyPredictedHex: string | null;
  cmyDeltaE00: number | null;
  cmyConfidence: Confidence;
  directToolhead: ToolheadId;
  selectedSpoolId: string;
  /** Explicit acknowledgement when the selected physical spool changes polymer family. */
  materialSubstitutionAcknowledged?: boolean;
}

export interface SourceColor {
  sourceSlots: string[];
  sourceMaterial: string;
  sourceHex: string;
}

export interface PlatePlan {
  id: string;
  /** Printable target rows are scheduled; blocked rows only expose omitted source units for resolution. */
  planningStatus: "printable" | "blocked";
  scopeId: string;
  /** Stable source units represented by this provisional target plate. */
  sourceUnitIds: string[];
  order: number;
  printer: PrinterKind;
  title: string;
  source: string;
  strategy: PrintStrategy;
  objectCount: number;
  loadoutLabel: string;
  loadoutColors: string[];
  logicalColorCount: number;
  /** Role-separated semantic source pairs retained for CMY+X and consent. */
  effectivePairCount?: number;
  /** Physical Direct identities after declared-profile role duplicates share a toolhead. */
  directPairCount?: number;
  /** Read-only original colors, available even when Direct Spools is unavailable. */
  sourceColors?: SourceColor[];
  recipeSummary: string;
  colorQuality: ColorQuality;
  estimatedDeltaE00: number;
  material: string;
  toolChanges: number | "Requires slicing";
  t4Action: string;
  warnings: string[];
  isFastMono: boolean;
  directEligible: boolean;
  directEligibilityReason?: string;
  mappings?: DirectColorMapping[];
}

export interface ProjectPlan {
  summary: ProjectSummary;
  alternativePlates: AlternativePlate[];
  /** Authoritative whole-project Direct Spools eligibility and source identities. */
  projectDirectPalette: ProjectDirectPalette;
  /**
   * User planning intent keyed by source scope. This remains stable when the
   * scheduler routes a Direct Spools or CMY+X mono job to the A1 mini.
   */
  scopeSelections: ScopeOverride[];
  unitPrinterSelections: UnitPrinterSelection[];
  /**
   * Server-generated fallback choices that need an explicit user decision.
   * Recipes and candidate identities are authoritative backend data; the UI
   * only records approval of the exact candidate it was shown.
   */
  colorResolutions: ColorResolution[];
  plates: PlatePlan[];
  batches: BatchSegment[];
  t4SwapCount: number;
  a1SpoolChangeCount: number;
  blockingErrors: string[];
  globalWarnings: string[];
  omittedUnitCount: number;
  planReady: boolean;
  /**
   * Canonical backend evidence for an explicitly approved partial conversion.
   * The frontend must return these records verbatim and never infer omissions.
   */
  partialConversion: PartialConversionAvailability;
  spools: PhysicalSpool[];
  currentLoadout: LoadedToolhead[];
  /** Currently loaded external spool; null means the first A1 setup is unknown. */
  currentA1SpoolId: string | null;
  /** Expected U1 state after every planned batch and restore action is complete. */
  plannedFinalLoadout: LoadedToolhead[];
  /** Expected A1 external spool after every planned A1 batch is complete. */
  plannedFinalA1SpoolId: string | null;
  restoreCmyByDefault: boolean;
  /** Applied backend mode for lossy per-plate Direct palette reduction. */
  customDirectPalettesEnabled: boolean;
}

export interface ProjectDirectPaletteReference {
  scopeId: string;
  scopeName: string;
  /** All aliases that the replan request must assign inside this scope. */
  requirementIds: string[];
}

export interface ProjectDirectPaletteMapping {
  id: string;
  /** Role-separated rows with the same value must use one physical spool/toolhead. */
  physicalIdentityId: string;
  sourceMaterial: string;
  sourceRole: string;
  sourceHex: string;
  sourceSlots: string[];
  sourceProfileIds: string[];
  usedBy: string[];
  references: ProjectDirectPaletteReference[];
  currentCmyxResults: ProjectDirectCmyxResult[];
  selectedSpoolId: string;
  directToolhead: ToolheadId;
  materialSubstitutionAcknowledged: boolean;
}

export interface ProjectDirectCmyxResult {
  scopeId: string;
  scopeName: string;
  recipe: string;
  predictedHex: string | null;
  deltaE00: number | null;
  confidence: Confidence;
}

export interface ProjectDirectPalette {
  available: boolean;
  /** Role-separated semantic rows retained for CMY+X comparison and consent. */
  effectivePairCount: number;
  /** Physical spool/toolhead identities subject to the four-toolhead limit. */
  directPairCount: number;
  maximumPairCount: number;
  unavailableReason: string | null;
  mappings: ProjectDirectPaletteMapping[];
}

export interface ProjectDirectPaletteChoice {
  mappingId: string;
  spoolId: string;
  materialSubstitutionAcknowledged: boolean;
}

export interface ExcludedSourceUnit {
  scopeId: string;
  planningUnitId: string;
  sourcePlateId: number | null;
  sourceUnitId: string;
  reason: string;
  errorIdentity: string;
}

export interface PartialConversionAvailability {
  available: boolean;
  exclusions: ExcludedSourceUnit[];
  reason: string;
}

export interface PartialConversionApproval {
  excludedSourceUnits: ExcludedSourceUnit[];
}

export interface AnalysisResult {
  plan: ProjectPlan;
  source: "tauri" | "browser-demo";
}

export interface ConversionAdapterCapability {
  target: string;
  adapterId: string;
  slicer: string;
  required: boolean;
  available: boolean;
  reason: string;
  report: unknown;
}

export interface ConversionCapability {
  available: boolean;
  reason: string;
  planFingerprint: string | null;
  sourceDialectSupport:
    "Supported" | "Experimental" | "Limited" | "Unsupported";
  experimentalDialectApprovalRequired: boolean;
  experimentalDialectFingerprint: string | null;
  adapters: ConversionAdapterCapability[];
}

export interface ExperimentalDialectApproval {
  sourceFingerprint: string;
}

export interface PreparedPhysicalSlot {
  toolhead: string;
  spoolId: string | null;
  spoolName: string | null;
  material: string | null;
  color: string | null;
  profile: string;
  settingId: string;
  filamentId: string;
}

export interface PreparedConversionArtifact {
  adapterId: string;
  target: string;
  printer: string;
  strategy: string;
  slicer: string;
  batchId: string;
  fileName: string;
  targetPlateIds: string[];
  sourceUnitIds: string[];
  loadout: PreparedPhysicalSlot[];
  setupActions: string[];
  adapterEvidence: unknown;
}

export interface ConversionPreparation {
  adapterId: string;
  sourceSha256: string;
  planFingerprint: string;
  bundleDirectoryName: string;
  artifacts: PreparedConversionArtifact[];
  warnings: string[];
  excludedSourceUnits: ExcludedSourceUnit[];
}

export interface PreparedConversion {
  preparationToken: string;
  conversionId: string;
  preparation: ConversionPreparation;
}

export interface PublishedConversionArtifact {
  adapterId: string;
  target: string;
  printer: string;
  strategy: string;
  slicer: string;
  batchId: string;
  fileName: string;
  relativePath: string;
  path: string;
  byteSize: number;
  sha256: string;
  plateCount: number;
  targetPlateIds: string[];
  sourceUnitIds: string[];
  loadout: PreparedPhysicalSlot[];
  setupActions: string[];
  validationStatus: string;
  adapterEvidence: unknown;
}

export interface ConversionResult {
  adapterId: string;
  outputDirectory: string;
  manifestPath: string;
  reportPath: string;
  artifacts: PublishedConversionArtifact[];
  warnings: string[];
  excludedSourceUnits: ExcludedSourceUnit[];
  warningsAcknowledged: boolean;
}

export type ConversionOutputActionKind = "open" | "reveal";

export interface ActiveConversionOutputAction {
  kind: ConversionOutputActionKind;
  path: string;
}

export interface ConversionProgress {
  conversionId: string;
  stage: string;
  message: string;
}

export type ConversionState =
  "active" | "cancelled" | "publishing" | "published";

export interface CancelConversionResult {
  conversionId: string;
  accepted: boolean;
  state: ConversionState | null;
}

export interface CancelAnalysisResult {
  accepted: boolean;
}

export interface ConfirmedSpool {
  id: string;
  name: string;
  colorName: string;
  hex: string;
  material: SpoolMaterial;
  sku?: string;
  profile?: string;
  colorBasis: "Measured" | "Nominal";
  available: boolean;
}

export interface FilamentLibraryDocument {
  schemaVersion: 2;
  spools: PhysicalSpool[];
}

/**
 * Rust serializes known variants as snake_case strings and custom variants as
 * externally tagged objects. The UI currently offers only qualified presets,
 * but it can still display records created by future native clients.
 */
export type CmyxSampleOrientation =
  "upright" | "flat" | "angled" | "unknown" | { custom: string };

export type CmyxGeometryClass =
  | "calibration_swatch"
  | "thin_wall"
  | "top_surface"
  | "volumetric"
  | "unknown"
  | { custom: string };

export interface CmyxGeometryContext {
  orientation: CmyxSampleOrientation;
  geometryClass: CmyxGeometryClass;
}

export type CmyxRecipeMode = "solid" | "cycle" | "ratio" | "match" | "gradient";

/** The guided measurement form intentionally omits one-component Solid. */
export type CmyxMeasurementRecipeMode = Exclude<CmyxRecipeMode, "solid">;

export interface CmyxRecipeComponent {
  /** One-based physical U1 toolhead slot. */
  slot: 1 | 2 | 3 | 4;
  /** Positive relative layer count. */
  weight: number;
}

export interface CmyxMixRecipe {
  mode: CmyxRecipeMode;
  /** Component order is the physical layer order and must be preserved. */
  components: CmyxRecipeComponent[];
}

export interface CmyxCalibrationLoadout {
  t1CalibrationId: string;
  t2CalibrationId: string;
  t3CalibrationId: string;
  t4CalibrationId: string;
}

export interface CmyxCalibrationPrinterProfile {
  printerModel: string;
  printerVariant: string;
  profileId: string;
  profileFingerprint: string;
}

export interface CmyxCalibrationContext {
  loadoutFingerprint: string;
  printerProfile: CmyxCalibrationPrinterProfile;
  nozzleDiameterMicrons: number;
  plateLayerHeightMicrons: number;
  subdivisionPolicy: string | Record<string, string>;
  subdivisionFactor: number;
  effectiveSublayerHeightMicrons: number;
  processFingerprint: string;
  orientation: CmyxSampleOrientation;
  geometryClass: CmyxGeometryClass;
}

export type CmyxMeasurementMethod =
  | "instrument_lab"
  | "instrument_srgb"
  | "reliable_manual_srgb"
  | "visual_swatch"
  | "legacy_unverified";

export type CmyxVerifiedMeasurementMethod = Exclude<
  CmyxMeasurementMethod,
  "legacy_unverified"
>;

export interface CmyxMeasurementProvenance {
  /** RFC 3339 instant; null only on safely migrated schema-v1 records. */
  measuredAt: string | null;
  method: CmyxMeasurementMethod;
  instrumentReference: string | null;
  operatorNotes: string | null;
}

export interface CmyxCalibrationRecord {
  id: string;
  loadout: CmyxCalibrationLoadout;
  context: CmyxCalibrationContext;
  recipe: CmyxMixRecipe;
  measuredOutputHex: string;
  provenance: CmyxMeasurementProvenance;
}

export interface CmyxCalibrationLibraryDocument {
  schemaVersion: 2;
  planningGeometryContext: CmyxGeometryContext;
  records: CmyxCalibrationRecord[];
}

/**
 * Deliberately excludes CalibrationContext. The native backend derives that
 * identity from its qualified U1 Full Spectrum process and the supplied
 * geometry, so nominal frontend data can never masquerade as a measurement.
 */
export interface CmyxCalibrationMeasurementInput {
  id: string;
  loadout: CmyxCalibrationLoadout;
  recipe: CmyxMixRecipe;
  measuredOutputHex: string;
  geometryContext: CmyxGeometryContext;
  provenance: {
    measuredAt: string;
    method: CmyxVerifiedMeasurementMethod;
    instrumentReference: string | null;
    operatorNotes: string | null;
  };
  /** Explicit UI acknowledgment; native code rejects false or missing values. */
  physicalMeasurementConfirmed: true;
}

export interface CmyxCalibrationProjectInput {
  projectId: string;
  /** Exact physical spool identities in T1, T2, T3, T4 order. */
  spoolIds: [string, string, string, string];
}

export interface CmyxCalibrationProjectValidation {
  valid: boolean;
  projectId: string | null;
  manifestSha256: string | null;
  swatchCount: number;
  fullSpectrum: {
    adapterId: string;
    valid: boolean;
    physicalFilamentCount: number;
    virtualFilamentCount: number;
    usedFilamentIds: number[];
    recipeCalibrationSampleIds?: string[];
    issues: Array<{ code: string; message: string }>;
  };
  issues: string[];
}

export interface CmyxCalibrationProjectResult {
  projectId: string;
  path: string;
  fileName: string;
  byteSize: number;
  artifactSha256: string;
  manifestPath: string;
  manifestSha256: string;
  swatchCount: number;
  productionQualified: boolean;
  validation: CmyxCalibrationProjectValidation;
  warnings: string[];
  nextSteps: string[];
}

export interface DirectAssignment {
  requirementId: string;
  spoolId: string;
  toolhead: ToolheadId | null;
  allowMaterialSubstitution?: boolean;
}

export interface ApprovedColorFallback {
  requirementId: string;
  candidateId: string;
}

export interface MaterialSubstitutionApproval {
  requirementId: string;
  candidateId: string;
  /** Exact source material label emitted by the backend, including ABS/ASA/TPU. */
  sourceMaterial: string;
  targetMaterial: SpoolMaterial;
  acknowledged: true;
}

export interface ColorResolution {
  scopeId: string;
  scopeName: string;
  requirementId: string;
  candidateId: string;
  sourceMaterial: string;
  sourceHex: string;
  targetMaterial: SpoolMaterial;
  targetHex: string;
  predictedHex: string | null;
  recipe: string;
  deltaE00: number | null;
  confidence: Confidence;
  requiredT4SpoolId: string | null;
  requiredT4Name: string | null;
  requiredT4Hex: string | null;
  colorApproved: boolean;
  materialApproved: boolean;
  requiresMaterialSubstitution: boolean;
  canAddDedicatedSpool: boolean;
  recommendation: string;
}

export interface ScopeOverride {
  scopeId: string;
  strategy: Extract<PrintStrategy, "cmyx" | "direct">;
  assignments: DirectAssignment[];
  approvedColorFallbacks: ApprovedColorFallback[];
  materialSubstitutions: MaterialSubstitutionApproval[];
}

export interface UnitPrinterSelection {
  sourceUnitId: string;
  preference: PrinterPreference;
}

export interface ReplanRequest {
  defaultStrategy: "auto" | "cmyx" | "direct";
  confirmedSpools: ConfirmedSpool[];
  scopeOverrides: ScopeOverride[];
  unitPrinterOverrides: UnitPrinterSelection[];
  currentLoadout: LoadedToolhead[];
  currentA1SpoolId: string | null;
  restoreCmyAfterDirect: boolean;
  allowDirectPaletteReduction: boolean;
  a1MiniEnabled: boolean;
  includedAlternativePlateIds: number[];
}

export interface BatchSegment {
  id: string;
  label: string;
  detail: string;
  strategy: PrintStrategy;
  startOrder: number;
  endOrder: number;
  plateCount: number;
  printer: PrinterKind;
  setupActions: string[];
  t4Change?: T4Change | null;
}

export interface T4Change {
  fromSpoolId: string | null;
  fromSpoolName: string | null;
  toSpoolId: string | null;
  toSpoolName: string | null;
}

export interface ToolheadAction {
  kind: "Keep" | "Unload" | "Load" | "Restore" | "Review";
  spoolName: string;
}
