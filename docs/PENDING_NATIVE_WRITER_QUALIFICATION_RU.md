# Отчёт о GUI qualification для A1 mini и U1 Full Spectrum

Дата выполнения: 3 августа 2026 года  
Статус: обязательные GUI-циклы завершены; оба production writer gate имеют
статус `qualified` для точных проверенных установок

## Что именно доказано

Программа создаёт редактируемые, ещё не нарезанные Project 3MF. Внутренний
validator доказывает целостность ZIP/OPC, геометрии, назначений материалов,
профилей и plate mapping. Выполненные циклы в целевых GUI-слайсерах отдельно
доказали, что точные проверенные версии:

1. открывают проекты без repair/incompatible-printer dialog;
2. успешно выполняют slice;
3. сохраняют целевые настройки и назначения;
4. после полного закрытия, повторного открытия и повторной нарезки сохраняют
   тот же семантический контракт.

Положительный результат не хранится как одиночный boolean. Каждый release
record связан с exact version и SHA-256 исполняемого файла, SHA-256 typed
qualification report и SHA-256 всех трёх файлов GUI-цикла. Изменение report,
record, профиля или артефакта закрывает gate.

## Проверенная локальная среда

| Компонент | Exact version | Executable SHA-256 |
|---|---:|---|
| Snapmaker Orca | 2.3.5 | `4c30e59cf582dcc4f12e43741fcab2f97045e972065d465481ed3760678d0fbe` |
| Bambu Studio | 02.02.00.85 | `88b72ac8523f36f4be0c5aafe99337baa6f596f502f9dfb4356ad2617c1b7458` |

Текущий статус проверяется командами:

```sh
cargo run -p u1-converter-cli -- doctor-full-spectrum \
  --orca-app "/Applications/Snapmaker Orca.app"

cargo run -p u1-converter-cli -- doctor-a1 \
  --bambu-app "/Applications/BambuStudio.app"
```

На точных проверенных установках обе команды должны вернуть `qualified` и
`conversionAvailable: true`. Несовпадение executable, effective profile pack
или embedded evidence fail-closed блокирует production conversion.

## A1 mini: выполненная матрица PLA и PETG

| Материал | Source SHA-256 | Candidate SHA-256 | GUI save 1 SHA-256 | GUI save 2 SHA-256 |
|---|---|---|---|---|
| PLA | `78a613193c05f96c77e0db5c7a5a0ce0eb051936752ae2b0def09ae8202cab8c` | `6ebe63cd19e12fec30b11e910bbf30f9abba0a37e8b3028c031e411be2d53c85` | `9561ee677e75710c22f01b1e01d925ec5cabdb70e525d8256f0cc11ad3646bdc` | `c1cb69848f3396fa9d649fcd31df32de662060cf3187c4b59af5e63187536f41` |
| PETG | `f0ad280964c0f0a3bdae1c3b8cc187d572da2986010f6e2c08e4e803db846b81` | `4c737de3a6b36ef453744f4fc2f4bf370b4c7a283ba1867e051cd033edf3d075` | `ca87bcb9baeaee56f1b01e378ed40216b5e231fbfd78962df5f4de0592835efd` | `a88feec5bd2490025191f001d00d943e51219f32c062bf96752facfee019f3ad` |

Оба кандидата использовали `Bambu Lab A1 mini 0.4 nozzle`, один внешний spool
и no-AMS contract. После каждого первого сохранения приложение было полностью
закрыто. Команда `validate-a1-round-trip` вернула `isValid: true` для обеих
последовательностей и подтвердила стабильность geometry/topology, parts,
transforms, placement, process и material identity.

Release record:
`crates/a1mini-adapter/qualification/a1mini-02.02.00.85.json`.
Hash-bound typed report:
`crates/a1mini-adapter/qualification/a1mini-02.02.00.85-report.json`.

Текущий adapter остаётся single-plate: приложение создаёт отдельный A1 mini
3MF для каждой целевой платы. Нативный multi-plate A1 output требует отдельной
будущей capability и своей qualification evidence; он не приписывается этому
gate.

## Snapmaker U1 Full Spectrum: выполненный GUI-цикл

Квалификация использовала актуальный heterogeneous-T4 writer candidate с
физическими CMY в T1–T3, Polymaker General PLA Black в T4, virtual mixed
definitions, solid T4 assignments и шестью платами.

| Evidence | SHA-256 |
|---|---|
| Source `Withered_Foxy.3mf` | `f0ad280964c0f0a3bdae1c3b8cc187d572da2986010f6e2c08e4e803db846b81` |
| Writer candidate | `df06f2f2ce4ac7a564066c13b2fe05ae579be7cfbd08832473268911c4fd997e` |
| GUI save 1 | `6cee93e41fe5b03fbd47f499decb97dd71ef453dc34f661a95be797fb74e42ed` |
| GUI save 2 | `50ef509e9e5bfadf834200ec110a2ff1cfa61569150a1cb6ed39100964a4dafa` |

Все шесть плат были нарезаны до первого сохранения и повторно после полного
закрытия и открытия приложения. `validate-full-spectrum-round-trip` вернул
`isValid: true` и подтвердил стабильность physical T1–T4 identity, virtual
definitions, decoded paint assignments, solid T4, plate maps,
subdivision/process globals, prime tower, geometry и placements.

Release record:
`crates/orca-adapter/qualification/u1-full-spectrum-2.3.5.json`.
Hash-bound typed report:
`crates/orca-adapter/qualification/u1-full-spectrum-2.3.5-report.json`.

## Граница writer qualification и физической точности цвета

Full Spectrum schema v2 намеренно разделяет два разных доказательства:

- глобальная writer qualification доказывает структуру native 3MF,
  совместимость с точным Snapmaker Orca и сохранение requested recipe;
- физическая точность оттенка остаётся per-user evidence для конкретных
  катушек, партии материала и print context.

Поэтому глобальный gate больше не содержит ложного
`physicalCouponPassed=true`. Его machine-readable поля фиксируют scope
`native_project_structure_and_gui_round_trip` и policy
`per_user_measured_calibration_or_explicit_color_approval`.

Для measured результата пользователь должен:

1. сгенерировать calibration chart для точного физического loadout T1–T4;
2. напечатать chart теми же CMY и T4, nozzle/process/plate context;
3. внести measured HEX для выбранного swatch;
4. перестроить план и проверить стабильный `calibrationSampleId` и точный
   calibration fingerprint в output/manifest.

Если измерения нет, planner может разрешить только явно подтверждённый
fingerprint-bound ближайший цвет и обязан показывать nominal confidence и
Delta E. Такое подтверждение разрешает напечатать выбранную пользователем
аппроксимацию, но не переименовывает её в measured calibration.

## Повторная проверка evidence

```sh
cargo run -p u1-converter-cli -- validate-a1-round-trip \
  candidate.3mf first-save.3mf reopened-save.3mf

cargo run -p u1-converter-cli -- validate-full-spectrum-round-trip \
  candidate.3mf first-save.3mf reopened-save.3mf
```

Любое изменение exact artifacts требует нового GUI-цикла и новых hash-bound
documents. Структурный test отдельно остаётся обязательным и не заменяет GUI
qualification или per-user physical color calibration.
