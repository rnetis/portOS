# Product Requirements Document: Next-Generation Port Operations Management System (PortOS)

**Version:** 1.0 (Draft) | **Date:** November 21, 2026 | **Status:** For review

## 1. Overview

### 1.1 Purpose

PortOS is a cloud-native, modular platform that unifies terminal operations, port community collaboration, regulatory workflows, and analytics in one system. It replaces fragmented legacy Terminal Operating Systems (TOS) and paper-based processes with real-time, data-driven, automated operations.

### 1.2 Problem Statement

Ports today suffer from siloed systems, manual planning, long truck queues, unpredictable vessel arrivals, slow customs clearance, limited visibility for stakeholders, and weak cybersecurity between IT and operational technology (OT). These drive congestion, high dwell times, safety incidents, and avoidable emissions.

### 1.3 Vision

A single, open, intelligent platform where every vessel, container, truck, and document is visible and optimized end to end, accessible to all stakeholders in their own language.

### 1.4 Goals and Success Metrics

| Goal | Target KPI |
| --- | --- |
| Reduce vessel waiting time | -25% average anchorage wait |
| Improve berth productivity | +15% crane moves per hour (gross) |
| Reduce truck turnaround | -30% gate-to-gate time |
| Reduce container dwell | -20% average yard dwell |
| Paperless clearance | 95% of documents processed digitally |
| Availability | 99.95% uptime for core operations |
| User adoption | 90% of stakeholders active within 12 months of go-live |

### 1.5 Non-Goals (v1)

- Replacing customs authority core systems (integrate only).
- Full autonomous terminal control without human override.
- Ship-board systems beyond data exchange (AIS, e-documents, arrival notices).

## 2. Users and Personas

| Persona | Primary needs |
| --- | --- |
| Port/Terminal Manager | KPI dashboards, capacity planning, performance and revenue visibility |
| Marine Operations Planner | Berth and vessel planning, pilot/tug scheduling |
| Yard Planner | Stack optimization, reefer and dangerous-goods control |
| Crane/Equipment Operator | Clear job instructions on mobile or in-cab terminals |
| Gate Clerk / Security Officer | Fast, accurate truck and container verification, access control |
| Shipping Line / Agent | Vessel schedules, bookings, status, billing |
| Trucking Company / Driver | Appointments, queue status, digital gate pass |
| Customs / Regulators | Manifests, risk flags, inspection scheduling |
| Finance / Billing Team | Tariffs, storage and demurrage, invoicing |
| IT / OT Administrator | Integration, security, monitoring, configuration |

## 3. Language and Localization Requirements (Bilingual: English and Persian)

The system **must be fully bilingual in English and Persian (Farsi)**, with equal quality in both languages across all user-facing surfaces.

### 3.1 Functional Requirements

- **L-1:** Full UI translation (web, mobile, kiosks, operator terminals) in English and Persian; users can switch language at any time without losing session state, and each user has a saved language preference.
- **L-2:** **Right-to-left (RTL) layout** for Persian with mirrored navigation, tables, forms, charts, and icons where directionality matters; left-to-right (LTR) remains for English. Mixed-direction text (for example, Persian text containing English container numbers or Latin codes) must render correctly using bidirectional (bidi) handling.
- **L-3:** **Persian calendar (Jalali/Solar Hijri)** supported alongside Gregorian for all date fields, reports, schedules, and invoices; users choose the display calendar, while storage and APIs remain in ISO 8601/UTC.
- **L-4:** **Numerals:** support Persian (۰۱۲۳۴۵۶۷۸۹) and Western digits by user preference; input accepts both and normalizes to a canonical form.
- **L-5:** Correct Persian typography: proper shaping, zero-width non-joiner (ZWNJ) handling, Arabic/Persian character normalization (for example, ی/ي and ک/ك), and approved Persian fonts.
- **L-6:** **Search and sorting** that work across both languages, with Persian collation and character-variant-insensitive matching.
- **L-7:** **Bilingual master data:** names of ports, vessels, companies, cargo types, and locations stored with English and Persian values where applicable.
- **L-8:** **Bilingual documents and outputs:** invoices, gate passes, delivery orders, reports, notifications (email, SMS, push), and error messages generated in the user's language, or both languages side by side on printed documents; PDF export must embed Persian fonts correctly.
- **L-9:** Multi-currency support with Iranian Rial/Toman handling and configurable display, plus major international currencies and exchange rate management.
- **L-10:** Translation managed through a resource-file/localization system so languages can be updated without code releases; the architecture must allow adding further languages later.
- **L-11:** Voice alerts and operator terminal messages available in both languages.

### 3.2 Non-Functional Localization Requirements

- Localization QA is part of every release (RTL visual regression testing, truncation/overflow checks).
- Persian and English content must have functional parity, with no feature available in only one language.
- Technical identifiers (container numbers per ISO 6346, IMO numbers, UN/LOCODE) remain in Latin script and are displayed LTR inside RTL text.

## 4. Functional Requirements

### 4.1 Vessel and Berth Management

- **F-1:** Vessel call registration with ETA/ETD, pre-arrival notices, and berth requests.
- **F-2:** AI-assisted berth allocation considering vessel size, draft, cargo, tide, crane availability, and priority rules.
- **F-3:** ETA prediction from AIS and weather data, with automatic re-planning when delays occur.
- **F-4:** Pilot, tug, and mooring crew scheduling and tracking.
- **F-5:** Anchorage and channel traffic management, with just-in-time arrival recommendations.
- **F-6:** Stowage plan (BAPLIE) import and discharge/load sequence planning.

### 4.2 Yard and Cargo Management

- **F-7:** Automated yard slot assignment and stack optimization to minimize reshuffles.
- **F-8:** Segregation rules for reefer, dangerous goods (IMDG), out-of-gauge, and empties.
- **F-9:** Real-time inventory of containers, break-bulk, bulk, and Ro-Ro cargo.
- **F-10:** Reefer monitoring (power, temperature alarms) with automated alerts.
- **F-11:** Container status management (holds, releases, inspections, damage records).
- **F-12:** Rail and barge intermodal planning.

### 4.3 Equipment and Workforce

- **F-13:** Real-time job dispatch to cranes, straddle carriers, tractors, and reach stackers; support for autonomous equipment and AGVs.
- **F-14:** Operator mobile and in-cab apps with task queues, confirmations, and issue reporting.
- **F-15:** Equipment availability, utilization, and shift/workforce planning.
- **F-16:** Predictive maintenance using IoT telemetry with automatic work-order creation.

### 4.4 Gate and Landside

- **F-17:** Truck appointment system with time-slot management and congestion-based slot release.
- **F-18:** Automated gate processing: OCR for container and plate recognition, RFID/QR, driver identity verification, and seal checks.
- **F-19:** Digital gate pass and paperless interchange (EIR) documents.
- **F-20:** Real-time truck queue visibility and driver notifications (SMS/app).
- **F-21:** Self-service kiosks and exception handling workflows.

### 4.5 Port Community System (Single Window)

- **F-22:** Shared portal for shipping lines, agents, forwarders, truckers, customs, and terminals.
- **F-23:** Electronic submission of manifests, booking data, delivery orders, and release orders.
- **F-24:** Integration with customs and regulatory authorities for clearance status, risk flags, and inspection scheduling.
- **F-25:** Support for electronic documents (e-B/L, e-certificates) with digital signatures.
- **F-26:** Event notifications and subscriptions (vessel arrival, container available, customs release).

### 4.6 Commercial and Billing

- **F-27:** Configurable tariff engine (per move, per day, per service, tiered, contract-based).
- **F-28:** Automatic charge calculation for storage, demurrage, detention, reefer, and special services.
- **F-29:** Invoicing, credit control, payment gateway integration, and reconciliation.
- **F-30:** Customer-facing billing portal with statements and dispute handling.

### 4.7 Visibility, Analytics, and Digital Twin

- **F-31:** Live operational dashboard with vessel, yard, gate, and equipment status.
- **F-32:** Configurable KPI dashboards (berth productivity, moves per hour, dwell time, truck turnaround, equipment utilization).
- **F-33:** Digital twin of the terminal for simulation, what-if scenarios, and bottleneck analysis.
- **F-34:** Historical analytics, scheduled reports, and data export.
- **F-35:** Container, vessel, and truck tracking for external stakeholders with role-based visibility.

### 4.8 Safety, Security, and Compliance

- **F-36:** ISPS-aligned access control with credentials, biometrics, and zone-based permissions.
- **F-37:** CCTV analytics for intrusion, PPE compliance, and collision/near-miss detection.
- **F-38:** Incident and accident logging, investigation workflow, and corrective actions.
- **F-39:** Dangerous goods compliance checks and emergency response plans linked to the live yard inventory.
- **F-40:** Full audit trail of user and system actions.

### 4.9 Sustainability

- **F-41:** Emissions tracking per vessel, equipment, and operation, with reporting.
- **F-42:** Shore power scheduling and energy consumption monitoring.
- **F-43:** Idle-time and fuel-burn reduction insights.

### 4.10 Administration and Configuration

- **F-44:** Role-based access control (RBAC) with fine-grained permissions and delegation.
- **F-45:** Configurable business rules, workflows, and approval chains without code changes.
- **F-46:** Multi-terminal and multi-port tenancy with data isolation.
- **F-47:** Master data management (customers, vessels, equipment, tariffs, locations).

## 5. Integration Requirements

- **I-1:** Open REST APIs with OpenAPI documentation and webhooks; event streaming (for example, Kafka-style) for real-time data.
- **I-2:** EDI standards support: EDIFACT (COPARN, CODECO, COARRI, BAPLIE, IFTMIN) and modern JSON equivalents.
- **I-3:** Integrations with AIS providers, weather and tide services, customs systems, banks/payment gateways, ERP/accounting, and rail/trucking systems.
- **I-4:** OT/IoT connectivity via OPC-UA, MQTT, and vendor equipment interfaces (cranes, PLCs, reefer monitoring, gate hardware).
- **I-5:** SSO and identity federation (SAML/OIDC) with national or corporate identity providers.

## 6. Non-Functional Requirements

### 6.1 Performance and Scalability

- Sub-second response for operational screens under normal load; gate transactions completed in under 5 seconds.
- Horizontal scaling to support multiple terminals and peak vessel-call periods.

### 6.2 Availability and Resilience

- 99.95% availability for critical operations; active-active or hot-standby deployment.
- **Offline resilience:** gate, yard, and equipment applications continue operating locally during network loss and synchronize on recovery; defined RPO of near zero and RTO under 15 minutes for core services.

### 6.3 Security

- Zero-trust architecture, encryption in transit and at rest, and strong authentication (MFA for privileged users).
- Network segmentation between IT and OT; secrets management; regular penetration testing and vulnerability management.
- Compliance alignment with ISPS, ISO 27001, IEC 62443 (industrial security), and applicable national data protection and data residency regulations.
- Support for on-premises or sovereign-cloud hosting where data residency is required.

### 6.4 Usability and Accessibility

- Responsive web and mobile apps; touch-friendly operator interfaces suitable for gloves and outdoor glare.
- Accessibility aligned with WCAG 2.1 AA in both English and Persian.
- Role-based, minimal-click workflows and in-app guidance.

### 6.5 Architecture and Deployment

- Cloud-native microservices with containerization and orchestration; deployable on-prem, private cloud, public cloud, or hybrid.
- API-first design, modular licensing (enable modules independently), and zero-downtime upgrades.
- Observability: centralized logging, metrics, tracing, and alerting.

### 6.6 Data and AI

- Unified data model with data governance, lineage, and retention policies.
- Explainable AI recommendations (berth, yard, ETA) with human override and feedback loops.

## 7. Release Plan (Phased)

| Phase | Scope | Indicative timeline |
| --- | --- | --- |
| **Phase 1: Foundation (MVP)** | Core platform, RBAC, bilingual UI/RTL, vessel and berth planning, yard management, gate automation, basic billing, dashboards | 0 to 9 months |
| **Phase 2: Community and Automation** | Port Community System, customs integration, appointment system, equipment dispatch, mobile apps, advanced billing | 9 to 18 months |
| **Phase 3: Intelligence** | Digital twin, predictive maintenance, AI optimization, autonomous equipment support, sustainability module | 18 to 30 months |

## 8. Assumptions, Dependencies, and Risks

**Assumptions**

- Port authority and terminal operators provide access to existing systems, data, and subject-matter experts.
- Network and hardware (cameras, OCR portals, RFID readers) are available or budgeted.

**Dependencies**

- Customs and regulatory agencies' API availability and cooperation.
- Equipment vendor interface documentation for OT integration.

**Key Risks and Mitigations**

| Risk | Mitigation |
| --- | --- |
| Legacy system migration disruption | Phased cutover, parallel run, rollback plan |
| Low stakeholder adoption | Early engagement, training, simple onboarding, bilingual support |
| Cybersecurity threats to OT | Segmentation, zero trust, continuous monitoring, incident response drills |
| RTL/bilingual UI defects | Dedicated localization QA, RTL design system from day one |
| Poor data quality | Data cleansing, validation rules, master data governance |
| Connectivity loss at terminal | Offline-first edge components and local caching |

## 9. Acceptance Criteria (High Level)

- All Phase 1 features operate fully in English and Persian with correct RTL/LTR rendering and Jalali/Gregorian calendar support.
- Gate transactions, berth plans, and yard moves meet the performance targets under load testing.
- Security assessment passed with no critical or high findings open.
- Failover and offline-mode tests passed.
- KPI baselines captured before go-live and measured after.

## 10. Open Questions

1. Which customs and national single-window systems must be integrated first?
2. Hosting model: on-premises, sovereign cloud, or hybrid?
3. Target terminal types (container, bulk, Ro-Ro, multipurpose) for the first release?
4. Existing TOS or ERP systems to migrate from or integrate with?
5. Required payment methods and currency/tariff rules (Rial/Toman handling)?
6. Regulatory and data residency constraints to confirm?
