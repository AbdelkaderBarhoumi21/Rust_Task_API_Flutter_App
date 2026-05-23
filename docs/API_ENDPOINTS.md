# Task API Endpoints Documentation

**Base URL:** `http://localhost:3000`

## Overview
This is a RESTful API for managing tasks, built with Axum and PostgreSQL. All endpoints support CORS with any origin.

---

## Endpoints

### 1. Get All Tasks
**GET** `/tasks`

Retrieves all tasks ordered by creation date (newest first).

**Response:**
- **Status:** `200 OK`
- **Body:**
```json
[
  {
    "id": "uuid",
    "title": "string",
    "description": "string",
    "priority": "Low | Medium | High",
    "status": "Pending | InProgress | Completed",
    "created_at": "timestamp",
    "completed_at": "timestamp | null"
  }
]
```

---

### 2. Get Single Task
**GET** `/tasks/:id`

Retrieves a specific task by its UUID.

**Parameters:**
- `id` (path parameter) - UUID of the task

**Response:**
- **Status:** `200 OK` - Task found
- **Body:**
```json
{
  "id": "uuid",
  "title": "string",
  "description": "string",
  "priority": "Low | Medium | High",
  "status": "Pending | InProgress | Completed",
  "created_at": "timestamp",
  "completed_at": "timestamp | null"
}
```

**Error Response:**
- **Status:** `404 NOT FOUND` - Task not found
- **Body:**
```json
{
  "message": "Task not found"
}
```

---

### 3. Create Task
**POST** `/tasks`

Creates a new task.

**Request Body:**
```json
{
  "title": "string (required)",
  "description": "string (required)",
  "priority": "Low | Medium | High (required)",
  "status": "Pending | InProgress | Completed (optional, defaults to Pending)"
}
```

**Response:**
- **Status:** `201 CREATED`
- **Body:**
```json
{
  "id": "uuid",
  "title": "string",
  "description": "string",
  "priority": "Low | Medium | High",
  "status": "Pending | InProgress | Completed",
  "created_at": "timestamp",
  "completed_at": "timestamp | null"
}
```

**Notes:**
- If status is set to `Completed` on creation, `completed_at` is automatically set to current time
- Task ID is automatically generated as UUID v4

---

### 4. Update Task
**PUT** `/tasks/:id`

Updates an existing task. All fields are optional.

**Parameters:**
- `id` (path parameter) - UUID of the task

**Request Body:**
```json
{
  "title": "string (optional)",
  "description": "string (optional)",
  "priority": "Low | Medium | High (optional)",
  "status": "Pending | InProgress | Completed (optional)"
}
```

**Response:**
- **Status:** `200 OK` - Task updated
- **Body:**
```json
{
  "id": "uuid",
  "title": "string",
  "description": "string",
  "priority": "Low | Medium | High",
  "status": "Pending | InProgress | Completed",
  "created_at": "timestamp",
  "completed_at": "timestamp | null"
}
```

**Error Response:**
- **Status:** `404 NOT FOUND` - Task not found
- **Body:**
```json
{
  "message": "Task not found"
}
```

**Notes:**
- When status changes to `Completed`, `completed_at` is automatically set to current time
- When status changes from `Completed` to another status, `completed_at` is cleared
- Unchanged fields retain their existing values

---

### 5. Delete Task
**DELETE** `/tasks/:id`

Deletes a task by its UUID.

**Parameters:**
- `id` (path parameter) - UUID of the task

**Response:**
- **Status:** `204 NO CONTENT` - Task successfully deleted

**Error Response:**
- **Status:** `404 NOT FOUND` - Task not found
- **Body:**
```json
{
  "message": "Task not found"
}
```

---

## Data Models

### Task Priority
- `Low`
- `Medium`
- `High`

### Task Status
- `Pending` - Task is pending
- `InProgress` - Task is in progress
- `Completed` - Task is completed

---

## Error Responses

All endpoints may return:

**500 INTERNAL SERVER ERROR**
```json
{
  "message": "Internal server error"
}
```

This occurs when there's a database error or other server-side issue.

---

## CORS
All endpoints support CORS with:
- **Allowed Origins:** Any (`*`)
- **Allowed Methods:** Any
- **Allowed Headers:** Any
