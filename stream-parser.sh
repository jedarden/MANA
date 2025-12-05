#!/bin/bash
# MANA Stream-JSON Parser
# Converts Claude Code stream-json output to human-readable format

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
WHITE='\033[1;37m'
GRAY='\033[0;90m'
NC='\033[0m' # No Color
BOLD='\033[1m'
DIM='\033[2m'

# Track state
CURRENT_TOOL=""
IN_THINKING=false

# Process each line
while IFS= read -r line; do
    # Skip empty lines
    [ -z "$line" ] && continue

    # Try to parse as JSON
    if ! echo "$line" | jq -e . >/dev/null 2>&1; then
        # Not JSON, print as-is
        echo "$line"
        continue
    fi

    # Extract type
    TYPE=$(echo "$line" | jq -r '.type // empty' 2>/dev/null)

    case "$TYPE" in
        "system")
            # System messages
            MSG=$(echo "$line" | jq -r '.message // empty' 2>/dev/null)
            if [ -n "$MSG" ]; then
                echo -e "${GRAY}[system] $MSG${NC}"
            fi
            ;;

        "assistant")
            # Assistant text output
            TEXT=$(echo "$line" | jq -r '.message.content // empty' 2>/dev/null)
            if [ -n "$TEXT" ] && [ "$TEXT" != "null" ]; then
                # Handle multi-line content
                echo "$TEXT" | while IFS= read -r text_line; do
                    echo -e "${WHITE}$text_line${NC}"
                done
            fi
            ;;

        "content_block_start")
            BLOCK_TYPE=$(echo "$line" | jq -r '.content_block.type // empty' 2>/dev/null)
            case "$BLOCK_TYPE" in
                "tool_use")
                    TOOL_NAME=$(echo "$line" | jq -r '.content_block.name // empty' 2>/dev/null)
                    CURRENT_TOOL="$TOOL_NAME"
                    echo -e "\n${CYAN}${BOLD}▶ Tool: $TOOL_NAME${NC}"
                    ;;
                "thinking")
                    IN_THINKING=true
                    echo -e "\n${MAGENTA}${DIM}💭 Thinking...${NC}"
                    ;;
                "text")
                    ;;
            esac
            ;;

        "content_block_delta")
            DELTA_TYPE=$(echo "$line" | jq -r '.delta.type // empty' 2>/dev/null)
            case "$DELTA_TYPE" in
                "text_delta")
                    TEXT=$(echo "$line" | jq -r '.delta.text // empty' 2>/dev/null)
                    if [ -n "$TEXT" ]; then
                        if [ "$IN_THINKING" = true ]; then
                            echo -ne "${MAGENTA}${DIM}$TEXT${NC}"
                        else
                            echo -ne "${WHITE}$TEXT${NC}"
                        fi
                    fi
                    ;;
                "input_json_delta")
                    # Tool input being streamed - show abbreviated
                    JSON=$(echo "$line" | jq -r '.delta.partial_json // empty' 2>/dev/null)
                    if [ -n "$JSON" ] && [ ${#JSON} -lt 100 ]; then
                        echo -ne "${GRAY}$JSON${NC}"
                    fi
                    ;;
            esac
            ;;

        "content_block_stop")
            if [ "$IN_THINKING" = true ]; then
                IN_THINKING=false
                echo -e "\n${MAGENTA}${DIM}💭 Done thinking${NC}"
            fi
            echo ""
            ;;

        "tool_use")
            # Tool being called
            TOOL=$(echo "$line" | jq -r '.name // empty' 2>/dev/null)
            echo -e "\n${CYAN}${BOLD}▶ Tool: $TOOL${NC}"

            # Show abbreviated input
            INPUT=$(echo "$line" | jq -r '.input // {}' 2>/dev/null | head -c 200)
            if [ -n "$INPUT" ]; then
                echo -e "${GRAY}Input: ${INPUT}...${NC}"
            fi
            ;;

        "tool_result")
            # Tool result
            TOOL_ID=$(echo "$line" | jq -r '.tool_use_id // empty' 2>/dev/null)
            IS_ERROR=$(echo "$line" | jq -r '.is_error // false' 2>/dev/null)
            CONTENT=$(echo "$line" | jq -r '.content // empty' 2>/dev/null)

            if [ "$IS_ERROR" = "true" ]; then
                echo -e "${RED}✗ Error:${NC}"
                echo "$CONTENT" | head -20 | while IFS= read -r result_line; do
                    echo -e "${RED}  $result_line${NC}"
                done
            else
                echo -e "${GREEN}✓ Result:${NC}"
                # Show first 20 lines of result
                echo "$CONTENT" | head -20 | while IFS= read -r result_line; do
                    echo -e "${GRAY}  $result_line${NC}"
                done

                # Check if truncated
                LINES=$(echo "$CONTENT" | wc -l)
                if [ "$LINES" -gt 20 ]; then
                    echo -e "${GRAY}  ... ($LINES total lines)${NC}"
                fi
            fi
            ;;

        "result")
            # Final result
            echo -e "\n${GREEN}${BOLD}═══ Result ═══${NC}"
            RESULT=$(echo "$line" | jq -r '.result // empty' 2>/dev/null)
            if [ -n "$RESULT" ]; then
                echo -e "${WHITE}$RESULT${NC}"
            fi

            # Show cost if available
            COST=$(echo "$line" | jq -r '.cost_usd // empty' 2>/dev/null)
            if [ -n "$COST" ] && [ "$COST" != "null" ]; then
                echo -e "${GRAY}Cost: \$$COST${NC}"
            fi

            # Show duration if available
            DURATION=$(echo "$line" | jq -r '.duration_ms // empty' 2>/dev/null)
            if [ -n "$DURATION" ] && [ "$DURATION" != "null" ]; then
                echo -e "${GRAY}Duration: ${DURATION}ms${NC}"
            fi
            ;;

        "error")
            # Error message
            ERROR=$(echo "$line" | jq -r '.error.message // .message // empty' 2>/dev/null)
            echo -e "${RED}${BOLD}ERROR: $ERROR${NC}"
            ;;

        "message_start"|"message_delta"|"message_stop")
            # Message lifecycle events - mostly ignore
            ;;

        *)
            # Unknown type - show raw if small
            if [ ${#line} -lt 200 ]; then
                echo -e "${GRAY}[${TYPE:-unknown}] $line${NC}"
            fi
            ;;
    esac
done
