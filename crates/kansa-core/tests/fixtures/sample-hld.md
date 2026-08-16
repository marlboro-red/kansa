# Intake Service — High-Level Design

## 1. Purpose

The intake service accepts applications from the public web form and the partner API. It validates them, assigns a case number, and hands them to the case-management queue. This document describes the intended behaviour; implementation details live in the ADRs.

## 2. Validation

All submissions shall be validated before a case number is issued. Validation is synchronous for the web form and asynchronous for the partner API (see §4).

- The email address shall match RFC 5322 and shall have a resolvable MX record.
- The ABN, when supplied, shall pass the ATO checksum. Invalid ABNs shall be rejected with error `ABN_INVALID`.
- Required fields are: applicant name, date of birth, email, and at least one contact number.
- Free-text fields shall be limited to 2,000 characters, e.g. the "circumstances" field.

Where the applicant is under 18, the system shall require a guardian's details. We expect this to be rare (approx. 2% of submissions).

### 2.1 Defaulting logic

When the preferred contact channel is omitted, the system shall default it to email. When both a mobile and a landline are supplied and no preference is stated, the system shall prefer mobile.

## 3. Lockout

While an applicant has failed identity verification 3 times within 24 hours, the system shall lock further attempts and shall notify the applicant through their verified channel. Locked accounts unlock automatically after 24 hours; support staff may unlock earlier.

| Event | Response | Owner |
|---|---|---|
| 3rd failed verification | Lock for 24 h | Platform |
| Manual unlock | Audit entry written | Support |

## 4. Partner API

The partner API is documented separately. Rate limits are per partner key: 100 requests/minute, burst 200. When a partner exceeds the limit, the API shall return HTTP 429 with `Retry-After`.

```json
{ "error": "RATE_LIMITED", "retryAfter": 30 }
```

Partners have asked whether limits could be raised for bulk migrations; this is under discussion. Is a per-partner override needed in v1? Unclear.

## 5. Non-goals

This design does not cover payment processing or document upload. Those are handled by the documents service and are out of scope here.
